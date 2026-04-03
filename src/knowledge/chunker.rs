// Copyright 2026 Muvon Un Limited
//
use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::config::KnowledgeConfig;
use crate::knowledge::content::{self, ContentType};
use crate::knowledge::types::KnowledgeChunk;

pub struct ContentChunker {
    config: KnowledgeConfig,
}

impl ContentChunker {
    pub fn new(config: KnowledgeConfig) -> Self {
        Self { config }
    }

    /// Extract text from any supported content type, then chunk.
    /// Returns (title, content_hash, chunks)
    pub fn extract_and_chunk(
        &self,
        source: &str,
        content_type: &ContentType,
        raw: &[u8],
    ) -> Result<(String, String, Vec<KnowledgeChunk>)> {
        match content_type {
            ContentType::Html => {
                let html = String::from_utf8_lossy(raw);
                self.parse_html_and_chunk(source, &html)
            }
            ContentType::Pdf => {
                let text = content::extract_text_from_pdf(raw)?;
                self.parse_text_and_chunk(source, &text)
            }
            ContentType::Docx => {
                let text = content::extract_text_from_docx(raw)?;
                self.parse_text_and_chunk(source, &text)
            }
            ContentType::Markdown => {
                let text = String::from_utf8_lossy(raw);
                self.parse_text_and_chunk(source, &text)
            }
            ContentType::PlainText => {
                let text = String::from_utf8_lossy(raw);
                self.parse_text_and_chunk(source, &text)
            }
        }
    }

    /// Parse plain text/markdown and chunk into semantic pieces.
    /// Returns (title, content_hash, chunks)
    fn parse_text_and_chunk(
        &self,
        source: &str,
        text: &str,
    ) -> Result<(String, String, Vec<KnowledgeChunk>)> {
        let title = self.extract_title_from_text(text);
        let content_hash = self.compute_hash(text);
        let chunks = self.chunk_markdown(source, &title, text)?;
        Ok((title, content_hash, chunks))
    }

    /// Extract title from text: first markdown heading, or first non-empty line (capped at 100 chars)
    fn extract_title_from_text(&self, text: &str) -> String {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Check for markdown heading
            if trimmed.starts_with('#') {
                return trimmed.trim_start_matches('#').trim().to_string();
            }
            // Use first non-empty line, capped
            let title: String = trimmed.chars().take(100).collect();
            return title;
        }
        "Untitled".to_string()
    }

    /// Parse HTML and chunk into semantic pieces
    /// Returns (title, content_hash, chunks)
    fn parse_html_and_chunk(
        &self,
        url: &str,
        html: &str,
    ) -> Result<(String, String, Vec<KnowledgeChunk>)> {
        // Try readability extraction first to strip nav/ads/boilerplate.
        // Falls back to raw HTML for pages that aren't article-like (API refs, indexes, etc.)
        let (title, clean_html) = self
            .extract_readable_content(html)
            .unwrap_or_else(|| (self.extract_title_from_html(html), html.to_string()));

        // Convert clean HTML to markdown
        let markdown = html2text::from_read(clean_html.as_bytes(), 120).unwrap_or_default();

        // Hash the clean content so cache is stable across nav/sidebar changes
        let content_hash = self.compute_hash(&markdown);

        // Parse markdown to detect section hierarchy and chunk
        let chunks = self.chunk_markdown(url, &title, &markdown)?;

        Ok((title, content_hash, chunks))
    }

    /// Extract main article content using Mozilla Readability algorithm.
    /// Returns (title, clean_html) or None if the page isn't article-like.
    fn extract_readable_content(&self, html: &str) -> Option<(String, String)> {
        let mut readability = dom_smoothie::Readability::new(html, None, None).ok()?;
        let article = readability.parse().ok()?;

        let title = article.title.trim().to_string();
        let title = if title.is_empty() {
            return None;
        } else {
            title
        };

        let clean_html = article.content.to_string();
        if clean_html.trim().is_empty() {
            return None;
        }

        Some((title, clean_html))
    }

    /// Fallback title extraction from raw HTML when readability fails
    fn extract_title_from_html(&self, html: &str) -> String {
        // Try <title> tag first
        if let Some(start) = html.find("<title>") {
            if let Some(end) = html[start..].find("</title>") {
                let title = &html[start + 7..start + end];
                let title = html2text::from_read(title.as_bytes(), 120).unwrap_or_default();
                let title = title.trim().to_string();
                if !title.is_empty() {
                    return title;
                }
            }
        }

        // Fallback to first <h1>
        if let Some(start) = html.find("<h1") {
            if let Some(content_start) = html[start..].find('>') {
                let content_start = start + content_start + 1;
                if let Some(end) = html[content_start..].find("</h1>") {
                    let title = &html[content_start..content_start + end];
                    let title = html2text::from_read(title.as_bytes(), 120).unwrap_or_default();
                    let title = title.trim().to_string();
                    if !title.is_empty() {
                        return title;
                    }
                }
            }
        }

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
            parent_content: None,
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
                // Section fits in one child — no parent needed
                let mut new_chunk = chunk;
                new_chunk.chunk_index = global_index;
                result.push(new_chunk);
                global_index += 1;
            } else {
                // Section is large: split into children, attach full section as parent.
                // Cap parent at 4× chunk_size so absurdly long sections don't bloat results.
                let header = self.extract_header(&chunk.content);
                let parent_text = {
                    let max = self.config.chunk_size * 4;
                    let cap =
                        self.floor_char_boundary(&chunk.content, chunk.content.len().min(max));
                    chunk.content[..cap].to_string()
                };
                let splits = self.split_text_with_overlap(&content_without_header);

                for (i, split) in splits.into_iter().enumerate() {
                    let child_content = format!("{}\n\n{}", header, split);
                    result.push(KnowledgeChunk {
                        id: uuid::Uuid::new_v4().to_string(),
                        source_url: chunk.source_url.clone(),
                        source_title: chunk.source_title.clone(),
                        chunk_index: global_index,
                        content: child_content,
                        parent_content: Some(parent_text.clone()),
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
        let chunker = ContentChunker::new(config);
        let html = "<html><head><title>Test Page</title></head><body></body></html>";
        let title = chunker.extract_title_from_html(html);
        assert_eq!(title, "Test Page");
    }

    #[test]
    fn test_extract_title_from_h1() {
        let config = KnowledgeConfig::default();
        let chunker = ContentChunker::new(config);
        let html = "<html><body><h1>Main Heading</h1></body></html>";
        let title = chunker.extract_title_from_html(html);
        assert_eq!(title, "Main Heading");
    }

    #[test]
    fn test_extract_title_fallback() {
        let config = KnowledgeConfig::default();
        let chunker = ContentChunker::new(config);
        let html = "<html><body><p>No title</p></body></html>";
        let title = chunker.extract_title_from_html(html);
        assert_eq!(title, "Untitled");
    }

    #[test]
    fn test_detect_header_level() {
        let config = KnowledgeConfig::default();
        let chunker = ContentChunker::new(config);
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
        let chunker = ContentChunker::new(config);
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
            parent_content: None,
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
