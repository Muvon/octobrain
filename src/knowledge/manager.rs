use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use std::sync::Arc;

use crate::config::{Config, KnowledgeConfig, SearchConfig};
use crate::embedding::EmbeddingProvider;
use crate::knowledge::chunker::HtmlChunker;
use crate::knowledge::store::KnowledgeStore;
use crate::knowledge::types::{IndexResult, KnowledgeSearchResult, KnowledgeStats};

pub struct KnowledgeManager {
    config: KnowledgeConfig,
    search_config: SearchConfig,
    store: KnowledgeStore,
    chunker: HtmlChunker,
    embedding_provider: Arc<dyn EmbeddingProvider>,
}

impl KnowledgeManager {
    pub async fn new(config: &Config) -> Result<Self> {
        let embedding_provider = crate::embedding::create_embedding_provider(config).await?;

        // Get vector dimension
        let test_embedding = embedding_provider.generate_embedding("test").await?;
        let vector_dim = test_embedding.len();

        let store = KnowledgeStore::new(vector_dim).await?;
        let chunker = HtmlChunker::new(config.knowledge.clone());

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
        // If source_url provided, check if needs indexing
        if let Some(url) = source_url {
            if self.needs_indexing(url).await? {
                self.index_url_internal(url).await?;
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
                source_url,
                self.config.max_results,
                use_hybrid,
            )
            .await
    }

    /// Check if URL needs indexing (not indexed or outdated)
    async fn needs_indexing(&self, url: &str) -> Result<bool> {
        match self.store.get_source_metadata(url).await? {
            None => Ok(true), // Not indexed
            Some((_, last_checked)) => {
                let outdating_duration = Duration::days(self.config.outdating_days as i64);
                let outdated = Utc::now() - last_checked > outdating_duration;
                Ok(outdated)
            }
        }
    }

    /// Index URL (public method for CLI)
    pub async fn index_url(&self, url: &str) -> Result<IndexResult> {
        // Check if already indexed and fresh
        if let Some((content_hash, last_checked)) = self.store.get_source_metadata(url).await? {
            let outdating_duration = Duration::days(self.config.outdating_days as i64);
            let is_fresh = Utc::now() - last_checked <= outdating_duration;

            if is_fresh {
                // Fetch to check if content changed
                let html = self.fetch_url(url).await?;
                let (_, new_hash, _) = self.chunker.parse_and_chunk(url, &html)?;

                if new_hash == content_hash {
                    // Content unchanged, just return cached
                    return Ok(IndexResult {
                        url: url.to_string(),
                        chunks_created: 0,
                        was_cached: true,
                        content_changed: false,
                    });
                }
            }
        }

        // Fetch and index
        let html = self.fetch_url(url).await?;
        let (title, content_hash, chunks) = self.chunker.parse_and_chunk(url, &html)?;

        if chunks.is_empty() {
            return Ok(IndexResult {
                url: url.to_string(),
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
            .store_chunks(url, &title, &content_hash, &chunks, &embeddings)
            .await?;

        Ok(IndexResult {
            url: url.to_string(),
            chunks_created: chunks.len(),
            was_cached: false,
            content_changed: true,
        })
    }

    /// Internal indexing (always reindexes if outdated)
    async fn index_url_internal(&self, url: &str) -> Result<()> {
        let html = self.fetch_url(url).await?;
        let (title, content_hash, chunks) = self.chunker.parse_and_chunk(url, &html)?;

        if chunks.is_empty() {
            return Ok(());
        }

        // Generate embeddings using proper batch API
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings =
            crate::embedding::generate_embeddings_batch(texts, self.embedding_provider.as_ref())
                .await?;

        self.store
            .store_chunks(url, &title, &content_hash, &chunks, &embeddings)
            .await?;

        Ok(())
    }

    /// Fetch URL content
    async fn fetch_url(&self, url: &str) -> Result<String> {
        // Basic URL validation
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

        let html = response
            .text()
            .await
            .context("Failed to read response body")?;
        Ok(html)
    }

    pub async fn delete_source(&self, url: &str) -> Result<()> {
        self.store.delete_source(url).await
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
