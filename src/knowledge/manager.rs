// Copyright 2026 Muvon Un Limited
//
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Config, KnowledgeConfig, SearchConfig};
use crate::embedding::EmbeddingProvider;
use crate::knowledge::chunker::ContentChunker;
use crate::knowledge::content::ContentType;
use crate::knowledge::store::KnowledgeStore;
use crate::knowledge::types::{IndexResult, KnowledgeSearchResult, KnowledgeStats};

/// Maximum source size in bytes (50 MB)
const MAX_SOURCE_SIZE: usize = 50 * 1024 * 1024;

pub struct KnowledgeManager {
    config: KnowledgeConfig,
    search_config: SearchConfig,
    store: KnowledgeStore,
    chunker: ContentChunker,
    embedding_provider: Arc<dyn EmbeddingProvider>,
}

impl KnowledgeManager {
    pub async fn new(config: &Config) -> Result<Self> {
        let embedding_provider = crate::embedding::create_embedding_provider(config).await?;

        // Get vector dimension
        let test_embedding = embedding_provider.generate_embedding("test").await?;
        let vector_dim = test_embedding.len();

        let store = KnowledgeStore::new(vector_dim).await?;
        let chunker = ContentChunker::new(config.knowledge.clone());

        Ok(Self {
            config: config.knowledge.clone(),
            search_config: config.search.clone(),
            store,
            chunker,
            embedding_provider: Arc::from(embedding_provider),
        })
    }

    /// Search knowledge base with on-demand indexing
    pub async fn search(
        &self,
        query: &str,
        source_url: Option<&str>,
    ) -> Result<Vec<KnowledgeSearchResult>> {
        // If source provided, normalize and check if needs indexing
        let normalized = source_url.map(normalize_source).transpose()?;
        let source_ref = normalized.as_deref();

        if let Some(source) = source_ref {
            if self.needs_indexing(source).await? {
                self.index_source_internal(source).await?;
            }
        }

        // Generate query embedding
        let query_embedding = self.embedding_provider.generate_embedding(query).await?;

        // Use global hybrid search flag
        let use_hybrid = self.search_config.hybrid.enabled;

        // Search with configurable limit and hybrid flag
        self.store
            .search(
                &query_embedding,
                query,
                source_ref,
                self.config.max_results,
                use_hybrid,
            )
            .await
    }

    /// Check if source needs indexing (not indexed or outdated)
    async fn needs_indexing(&self, source: &str) -> Result<bool> {
        match self.store.get_source_metadata(source).await? {
            None => Ok(true), // Not indexed
            Some((_, last_checked)) => {
                if is_local_source(source) {
                    // Local files: compare file mtime vs last_checked
                    let path = source_to_path(source)?;
                    let metadata = tokio::fs::metadata(&path)
                        .await
                        .context("Failed to read file metadata")?;
                    let mtime: DateTime<Utc> = metadata.modified()?.into();
                    Ok(mtime > last_checked)
                } else {
                    // HTTP: use outdating_days
                    let outdating_duration = Duration::days(self.config.outdating_days as i64);
                    let outdated = Utc::now() - last_checked > outdating_duration;
                    Ok(outdated)
                }
            }
        }
    }

    /// Index a source (public method for CLI). Accepts URLs and file paths.
    pub async fn index_source(&self, source: &str) -> Result<IndexResult> {
        let source = normalize_source(source)?;

        // Check if already indexed and fresh
        if let Some((content_hash, last_checked)) = self.store.get_source_metadata(&source).await? {
            let is_fresh = if is_local_source(&source) {
                let path = source_to_path(&source)?;
                let metadata = tokio::fs::metadata(&path)
                    .await
                    .context("Failed to read file metadata")?;
                let mtime: DateTime<Utc> = metadata.modified()?.into();
                mtime <= last_checked
            } else {
                let outdating_duration = Duration::days(self.config.outdating_days as i64);
                Utc::now() - last_checked <= outdating_duration
            };

            if is_fresh {
                // Fetch to check if content changed
                let (content_type, bytes) = self.fetch_source(&source).await?;
                let (_, new_hash, _) =
                    self.chunker
                        .extract_and_chunk(&source, &content_type, &bytes)?;

                if new_hash == content_hash {
                    return Ok(IndexResult {
                        url: source,
                        chunks_created: 0,
                        was_cached: true,
                        content_changed: false,
                    });
                }
            }
        }

        // Fetch and index
        let (content_type, bytes) = self.fetch_source(&source).await?;
        let (title, content_hash, chunks) =
            self.chunker
                .extract_and_chunk(&source, &content_type, &bytes)?;

        if chunks.is_empty() {
            return Ok(IndexResult {
                url: source,
                chunks_created: 0,
                was_cached: false,
                content_changed: true,
            });
        }

        // Generate embeddings using proper batch API
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings =
            crate::embedding::generate_embeddings_batch(texts, self.embedding_provider.as_ref())
                .await?;

        // Store
        self.store
            .store_chunks(&source, &title, &content_hash, &chunks, &embeddings)
            .await?;

        Ok(IndexResult {
            url: source,
            chunks_created: chunks.len(),
            was_cached: false,
            content_changed: true,
        })
    }

    /// Internal indexing (always reindexes if outdated)
    async fn index_source_internal(&self, source: &str) -> Result<()> {
        let (content_type, bytes) = self.fetch_source(source).await?;
        let (title, content_hash, chunks) =
            self.chunker
                .extract_and_chunk(source, &content_type, &bytes)?;

        if chunks.is_empty() {
            return Ok(());
        }

        // Generate embeddings using proper batch API
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings =
            crate::embedding::generate_embeddings_batch(texts, self.embedding_provider.as_ref())
                .await?;

        self.store
            .store_chunks(source, &title, &content_hash, &chunks, &embeddings)
            .await?;

        Ok(())
    }

    /// Fetch source content as raw bytes with content type detection.
    async fn fetch_source(&self, source: &str) -> Result<(ContentType, Vec<u8>)> {
        if is_local_source(source) {
            let path = source_to_path(source)?;

            let metadata = tokio::fs::metadata(&path)
                .await
                .with_context(|| format!("File not found: {}", path.display()))?;

            if metadata.len() as usize > MAX_SOURCE_SIZE {
                anyhow::bail!(
                    "File too large: {} bytes (max {} bytes)",
                    metadata.len(),
                    MAX_SOURCE_SIZE
                );
            }

            let bytes = tokio::fs::read(&path)
                .await
                .with_context(|| format!("Failed to read file: {}", path.display()))?;

            let content_type = ContentType::from_extension(path.to_str().unwrap_or(""))
                .unwrap_or(ContentType::PlainText);

            Ok((content_type, bytes))
        } else {
            self.fetch_url_bytes(source).await
        }
    }

    /// Fetch URL content as raw bytes with content type detection from headers.
    async fn fetch_url_bytes(&self, url: &str) -> Result<(ContentType, Vec<u8>)> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            anyhow::bail!("URL cannot be empty");
        }

        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            anyhow::bail!(
                "Invalid URL: must start with http:// or https://, got: {}",
                trimmed
            );
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Octobrain/1.0")
            .build()?;

        let response = client
            .get(url)
            .send()
            .await
            .context("Failed to fetch URL")?;

        if !response.status().is_success() {
            anyhow::bail!("HTTP error: {}", response.status());
        }

        // Detect content type from Content-Type header, fall back to URL extension, then Html
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(ContentType::from_content_type_header)
            .or_else(|| ContentType::from_extension(url))
            .unwrap_or(ContentType::Html);

        let bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?;

        if bytes.len() > MAX_SOURCE_SIZE {
            anyhow::bail!(
                "Response too large: {} bytes (max {} bytes)",
                bytes.len(),
                MAX_SOURCE_SIZE
            );
        }

        Ok((content_type, bytes.to_vec()))
    }

    pub async fn delete_source(&self, source: &str) -> Result<()> {
        let source = normalize_source(source)?;
        self.store.delete_source(&source).await
    }

    pub async fn get_stats(&self) -> Result<KnowledgeStats> {
        self.store.get_stats().await
    }

    pub async fn list_sources(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, usize, chrono::DateTime<chrono::Utc>)>> {
        self.store.list_sources(limit).await
    }
}

// ============================================================================
// Source helpers
// ============================================================================

/// Check if a source string refers to a local file
fn is_local_source(source: &str) -> bool {
    source.starts_with("file://") || source.starts_with('/')
}

/// Normalize a source string to a canonical form.
/// - HTTP URLs pass through unchanged
/// - Local paths (absolute, relative, ~/...) become file:///absolute/path
fn normalize_source(source: &str) -> Result<String> {
    let trimmed = source.trim();

    // Already a URL
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }

    // Already a file:// URI
    if trimmed.starts_with("file://") {
        return Ok(trimmed.to_string());
    }

    // Resolve to absolute path
    let path = if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        home.join(rest)
    } else {
        let p = PathBuf::from(trimmed);
        if p.is_relative() {
            std::env::current_dir()?.join(p)
        } else {
            p
        }
    };

    // Canonicalize to resolve symlinks and ..
    let canonical = path
        .canonicalize()
        .with_context(|| format!("File not found: {}", path.display()))?;

    Ok(format!("file://{}", canonical.display()))
}

/// Convert a normalized source string to a filesystem path
fn source_to_path(source: &str) -> Result<PathBuf> {
    if let Some(rest) = source.strip_prefix("file://") {
        Ok(PathBuf::from(rest))
    } else if source.starts_with('/') {
        Ok(PathBuf::from(source))
    } else {
        anyhow::bail!("Not a local source: {}", source)
    }
}
