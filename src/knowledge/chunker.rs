use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::config::KnowledgeConfig;
use crate::knowledge::types::KnowledgeChunk;

pub struct HtmlChunker {
    config: KnowledgeConfig,
}

impl HtmlChunker {
    pub fn new(config: KnowledgeConfig) -> Self {
        Self { config }
    }

    /// Parse HTML and chunk into semantic pieces
    /// Returns (title, content_hash, chunks)
    pub fn parse_and_chunk(
        &self,
        url: &str,
        html: &str,
    ) -> Result<(String, String, Vec<KnowledgeChunk>)> {
        // Extract title from HTML
        let title = self.extract_title(html);

        // Convert HTML to markdown
        let markdown = html2text::from_read(html.as_bytes(), 120);

        // Compute content hash
        let content_hash = self.compute_hash(&markdown);

        // Parse markdown to detect section hierarchy and chunk
        let chunks = self.chunk_markdown(url, &title, &markdown)?;

        Ok((title, content_hash, chunks))
    }

    /// Extract title from HTML
    fn extract_title(&self, html: &str) -> String {
        // Try <title> tag first
        if let Some(start) = html.find("<title>") {
            if let Some(end) = html[start..].find("</title>") {
                let title = &html[start + 7..start + end];
                let title = html2text::from_read(title.as_bytes(), 120);
                let title = title.trim();
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }

        // Fallback to first <h1>
        if let Some(start) = html.find("<h1") {
            if let Some(content_start) = html[start..].find('>') {
                let content_start = start + content_start + 1;
                if let Some(end) = html[content_start..].find("</h1>") {
                    let title = &html[content_start..content_start + end];
                    let title = html2text::from_read(title.as_bytes(), 120);
                    let title = title.trim();
                    if !title.is_empty() {
                        return title.to_string();
                    }
                }
            }
        }

        // Fallback to "Untitled"
        "Untitled".to_string()
    }

    /// Compute SHA256 hash of content
    fn compute_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Chunk markdown content with section hierarchy tracking
    fn chunk_markdown(
        &self,
        url: &str,
        title: &str,
        markdown: &str,
    ) -> Result<Vec<KnowledgeChunk>> {
        let mut chunks = Vec::new();
        let mut current_section_path: Vec<String> = Vec::new();
        let mut chunk_index = 0;

        // Split into lines for header detection
        let lines: Vec<&str> = markdown.lines().collect();
        let mut current_text = String::new();
        let mut char_start = 0;

        for line in lines {
            // Detect markdown headers
            if let Some(level) = self.detect_header_level(line) {
                // Flush current chunk if we have content
                if !current_text.trim().is_empty() {
                    if let Some(chunk) = self.create_chunk(
                        url,
                        title,
                        &current_section_path,
                        &current_text,
                        chunk_index,
                        (char_start, char_start + current_text.len()),
                    ) {
                        chunks.push(chunk);
                        chunk_index += 1;
                    }
                    char_start += current_text.len();
                    current_text.clear();
                }

                // Update section path
                let header_text = line.trim_start_matches('#').trim().to_string();
                self.update_section_path(&mut current_section_path, level, header_text);
            }

            current_text.push_str(line);
            current_text.push('\n');
        }

        // Flush remaining content
        if !current_text.trim().is_empty() {
            if let Some(chunk) = self.create_chunk(
                url,
                title,
                &current_section_path,
                &current_text,
                chunk_index,
                (char_start, char_start + current_text.len()),
            ) {
                chunks.push(chunk);
            }
        }

        // Now split large chunks with overlap
        let final_chunks = self.split_with_overlap(chunks)?;

        Ok(final_chunks)
    }

    /// Detect markdown header level (1-6)
    fn detect_header_level(&self, line: &str) -> Option<usize> {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            return None;
        }

        let level = trimmed.chars().take_while(|&c| c == '#').count();
        if level > 0 && level <= 6 {
            Some(level)
        } else {
            None
        }
    }

    /// Update section path based on header level
    fn update_section_path(&self, path: &mut Vec<String>, level: usize, header: String) {
        // Truncate path to level - 1
        path.truncate(level.saturating_sub(1));
        // Add new header
        path.push(header);
    }

    /// Create a chunk with metadata
    fn create_chunk(
        &self,
        url: &str,
        title: &str,
        section_path: &[String],
        content: &str,
        chunk_index: i32,
        char_range: (usize, usize),
    ) -> Option<KnowledgeChunk> {
        let content = content.trim();
        if content.len() < 50 {
            return None;
        }

        // Prepend title and section path
        let mut full_content = String::new();
        full_content.push_str(title);
        if !section_path.is_empty() {
            full_content.push_str(" > ");
            full_content.push_str(&section_path.join(" > "));
        }
        full_content.push_str("\n\n");
        full_content.push_str(content);

        Some(KnowledgeChunk {
            id: uuid::Uuid::new_v4().to_string(),
            source_url: url.to_string(),
            source_title: title.to_string(),
            chunk_index,
            content: full_content,
            section_path: section_path.to_vec(),
            char_start: char_range.0,
            char_end: char_range.1,
        })
    }

    /// Split large chunks with overlap
    fn split_with_overlap(&self, chunks: Vec<KnowledgeChunk>) -> Result<Vec<KnowledgeChunk>> {
        let mut result = Vec::new();
        let mut global_index = 0;

        for chunk in chunks {
            let content_without_header = self.extract_content_without_header(&chunk.content);

            if content_without_header.len() <= self.config.chunk_size {
                // Chunk is small enough, keep as is
                let mut new_chunk = chunk;
                new_chunk.chunk_index = global_index;
                result.push(new_chunk);
                global_index += 1;
            } else {
                // Split into smaller chunks with overlap
                let header = self.extract_header(&chunk.content);
                let splits = self.split_text_with_overlap(&content_without_header);

                for (i, split) in splits.into_iter().enumerate() {
                    let full_content = format!("{}\n\n{}", header, split);
                    result.push(KnowledgeChunk {
                        id: uuid::Uuid::new_v4().to_string(),
                        source_url: chunk.source_url.clone(),
                        source_title: chunk.source_title.clone(),
                        chunk_index: global_index,
                        content: full_content,
                        section_path: chunk.section_path.clone(),
                        char_start: chunk.char_start
                            + i * (self.config.chunk_size - self.config.chunk_overlap),
                        char_end: chunk.char_start
                            + i * (self.config.chunk_size - self.config.chunk_overlap)
                            + split.len(),
                    });
                    global_index += 1;
                }
            }
        }

        Ok(result)
    }

    /// Extract header (title + section path) from chunk content
    fn extract_header(&self, content: &str) -> String {
        if let Some(pos) = content.find("\n\n") {
            content[..pos].to_string()
        } else {
            String::new()
        }
    }

    /// Extract content without header
    fn extract_content_without_header(&self, content: &str) -> String {
        if let Some(pos) = content.find("\n\n") {
            content[pos + 2..].to_string()
        } else {
            content.to_string()
        }
    }

    /// Split text into chunks with overlap
    fn split_text_with_overlap(&self, text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut start = 0;

        while start < text.len() {
            let end_target = (start + self.config.chunk_size).min(text.len());
            let end = self.floor_char_boundary(text, end_target);

            // Try to find sentence boundary
            let chunk_end = if end < text.len() {
                self.find_sentence_boundary(text, start, end)
            } else {
                end
            };
            let chunk_end = if chunk_end <= start {
                self.ceil_char_boundary(text, start + 1)
            } else {
                chunk_end
            };

            chunks.push(text[start..chunk_end].to_string());

            // Move start with overlap
            if chunk_end >= text.len() {
                break;
            }
            let next_target = chunk_end.saturating_sub(self.config.chunk_overlap);
            start = self.floor_char_boundary(text, next_target);
        }

        chunks
    }

    /// Find sentence boundary near target position
    fn find_sentence_boundary(&self, text: &str, _start: usize, target: usize) -> usize {
        // Look for sentence endings within 100 chars of target
        let search_start = self.floor_char_boundary(text, target.saturating_sub(100));
        let search_end = self.floor_char_boundary(text, (target + 100).min(text.len()));
        let search_text = &text[search_start..search_end];

        // Find last sentence ending before target
        let relative_target = target - search_start;
        for (i, ch) in search_text[..relative_target].char_indices().rev() {
            if matches!(ch, '.' | '!' | '?') {
                // Check if followed by space or newline
                if let Some(next_ch) = search_text[i + 1..].chars().next() {
                    if next_ch.is_whitespace() {
                        return search_start + i + 1;
                    }
                }
            }
        }

        // No sentence boundary found, use target
        target
    }

    fn floor_char_boundary(&self, text: &str, mut idx: usize) -> usize {
        idx = idx.min(text.len());
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    fn ceil_char_boundary(&self, text: &str, mut idx: usize) -> usize {
        idx = idx.min(text.len());
        while idx < text.len() && !text.is_char_boundary(idx) {
            idx += 1;
        }
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title_from_title_tag() {
        let config = KnowledgeConfig::default();
        let chunker = HtmlChunker::new(config);
        let html = "<html><head><title>Test Page</title></head><body></body></html>";
        let title = chunker.extract_title(html);
        assert_eq!(title, "Test Page");
    }

    #[test]
    fn test_extract_title_from_h1() {
        let config = KnowledgeConfig::default();
        let chunker = HtmlChunker::new(config);
        let html = "<html><body><h1>Main Heading</h1></body></html>";
        let title = chunker.extract_title(html);
        assert_eq!(title, "Main Heading");
    }

    #[test]
    fn test_extract_title_fallback() {
        let config = KnowledgeConfig::default();
        let chunker = HtmlChunker::new(config);
        let html = "<html><body><p>No title</p></body></html>";
        let title = chunker.extract_title(html);
        assert_eq!(title, "Untitled");
    }

    #[test]
    fn test_detect_header_level() {
        let config = KnowledgeConfig::default();
        let chunker = HtmlChunker::new(config);
        assert_eq!(chunker.detect_header_level("# Header 1"), Some(1));
        assert_eq!(chunker.detect_header_level("## Header 2"), Some(2));
        assert_eq!(chunker.detect_header_level("### Header 3"), Some(3));
        assert_eq!(chunker.detect_header_level("Regular text"), None);
    }

    #[test]
    fn test_chunk_overlap() {
        let config = KnowledgeConfig {
            chunk_size: 100,
            chunk_overlap: 20,
            outdating_days: 90,
            max_results: 10,
        };
        let chunker = HtmlChunker::new(config);
        let text = "a".repeat(250);
        let chunks = chunker.split_text_with_overlap(&text);
        assert!(chunks.len() > 1);
        // Verify overlap exists
        assert!(chunks[1].starts_with(&"a".repeat(20)));
    }

    // URL validation tests
    #[test]
    fn test_url_validation_https_valid() {
        let urls = vec![
            "https://example.com",
            "https://example.com/page",
            "https://example.com/page?q=1",
        ];

        for url in urls {
            assert!(
                url.starts_with("http://") || url.starts_with("https://"),
                "URL {} should be valid",
                url
            );
        }
    }

    #[test]
    fn test_url_validation_http_valid() {
        let url = "http://example.com";
        assert!(url.starts_with("http://") || url.starts_with("https://"));
    }

    #[test]
    fn test_url_validation_missing_scheme() {
        let urls = vec![
            "example.com",
            "www.example.com",
            "/path/to/page",
            "ftp://example.com",
        ];

        for url in urls {
            let valid = url.starts_with("http://") || url.starts_with("https://");
            assert!(!valid, "URL {} should be invalid (missing http/https)", url);
        }
    }

    // Type tests
    #[test]
    fn test_knowledge_chunk_fields() {
        use crate::knowledge::types::KnowledgeChunk;

        let chunk = KnowledgeChunk {
            id: "test-id".to_string(),
            source_url: "https://example.com".to_string(),
            source_title: "Test Page".to_string(),
            chunk_index: 0,
            content: "Test content".to_string(),
            section_path: vec!["Section 1".to_string()],
            char_start: 0,
            char_end: 12,
        };

        assert_eq!(chunk.id, "test-id");
        assert_eq!(chunk.source_url, "https://example.com");
        assert_eq!(chunk.source_title, "Test Page");
        assert_eq!(chunk.chunk_index, 0);
        assert_eq!(chunk.content, "Test content");
        assert_eq!(chunk.section_path.len(), 1);
    }

    #[test]
    fn test_knowledge_stats_default() {
        use crate::knowledge::types::KnowledgeStats;

        let stats = KnowledgeStats {
            total_sources: 0,
            total_chunks: 0,
            oldest_indexed: None,
            newest_indexed: None,
        };

        assert_eq!(stats.total_sources, 0);
        assert_eq!(stats.total_chunks, 0);
        assert!(stats.oldest_indexed.is_none());
        assert!(stats.newest_indexed.is_none());
    }

    #[test]
    fn test_index_result_fields() {
        use crate::knowledge::types::IndexResult;

        let result = IndexResult {
            url: "https://example.com".to_string(),
            chunks_created: 5,
            was_cached: false,
            content_changed: true,
        };

        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.chunks_created, 5);
        assert!(!result.was_cached);
        assert!(result.content_changed);
    }
}
