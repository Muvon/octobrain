// Copyright 2026 Muvon Un Limited
//
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{Config, KnowledgeConfig, SearchConfig};
use crate::embedding::EmbeddingProvider;
use crate::knowledge::chunker::ContentChunker;
use crate::knowledge::content::ContentType;
use crate::knowledge::store::KnowledgeStore;
use crate::knowledge::types::{
    IndexResult, KnowledgeChunk, KnowledgeSearchResult, KnowledgeStats, MatchResult, ReadResult,
    StoreResult,
};

/// Maximum source size in bytes (50 MB)
const MAX_SOURCE_SIZE: usize = 50 * 1024 * 1024;

pub struct KnowledgeManager {
    config: KnowledgeConfig,
    search_config: SearchConfig,
    store: KnowledgeStore,
    chunker: ContentChunker,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    embedding_timeout_secs: u64,
}

impl KnowledgeManager {
    pub async fn new(config: &Config) -> Result<Self> {
        let embedding_provider = crate::embedding::create_embedding_provider(config).await?;

        // Get vector dimension
        let test_embedding = crate::embedding::generate_embedding(
            "test",
            embedding_provider.as_ref(),
            config.embedding.timeout_secs,
        )
        .await?;
        let vector_dim = test_embedding.len();

        let store = KnowledgeStore::new(vector_dim).await?;
        let chunker = ContentChunker::new(config.knowledge.clone());

        // Clean up expired session-scoped chunks (crash recovery)
        store
            .cleanup_expired_sessions(config.knowledge.session_ttl_hours)
            .await
            .ok();

        Ok(Self {
            config: config.knowledge.clone(),
            search_config: config.search.clone(),
            store,
            chunker,
            embedding_provider: Arc::from(embedding_provider),
            embedding_timeout_secs: config.embedding.timeout_secs,
        })
    }

    /// Embed and store a document's chunks. Returns the number of chunks stored.
    /// Shared ingest tail used by every indexer (URL, file, box, stored note).
    async fn embed_and_store(
        &self,
        source: &str,
        title: &str,
        scope: &str,
        content_hash: &str,
        chunks: Vec<KnowledgeChunk>,
        session_id: Option<&str>,
    ) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }

        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = crate::embedding::generate_embeddings_batch(
            texts,
            self.embedding_provider.as_ref(),
            self.embedding_timeout_secs,
        )
        .await?;

        self.store
            .store_chunks(
                source,
                title,
                scope,
                content_hash,
                &chunks,
                &embeddings,
                session_id,
            )
            .await?;

        Ok(chunks.len())
    }

    /// Rerank knowledge results with the configured cross-encoder, degrading
    /// gracefully to the pre-rerank order (truncated to `top_n`) on any failure.
    /// The reranked document text is the parent section when present (richer
    /// signal), else the chunk content.
    async fn rerank(
        &self,
        query: &str,
        mut results: Vec<KnowledgeSearchResult>,
        top_n: usize,
    ) -> Vec<KnowledgeSearchResult> {
        let cfg = &self.search_config.reranker;
        let (provider, model) = match cfg.model.split_once(':') {
            Some(pm) => pm,
            None => {
                tracing::warn!("Invalid reranker model '{}'; skipping rerank", cfg.model);
                results.truncate(top_n);
                return results;
            }
        };

        let documents: Vec<String> = results
            .iter()
            .map(|r| {
                r.chunk
                    .parent_content
                    .clone()
                    .unwrap_or_else(|| r.chunk.content.clone())
            })
            .collect();

        let rerank_fut = octolib::reranker::rerank(query, documents, provider, model, Some(top_n));
        let outcome = if cfg.timeout_secs == 0 {
            rerank_fut.await
        } else {
            match tokio::time::timeout(std::time::Duration::from_secs(cfg.timeout_secs), rerank_fut)
                .await
            {
                Ok(inner) => inner,
                Err(_) => Err(anyhow::anyhow!(
                    "Reranker timed out after {}s",
                    cfg.timeout_secs
                )),
            }
        };

        match outcome {
            Ok(response) => {
                let mut reranked = Vec::with_capacity(response.results.len());
                for rr in response.results {
                    if let Some(original) = results.get(rr.index) {
                        let mut hit = original.clone();
                        hit.relevance_score = rr.relevance_score as f32;
                        reranked.push(hit);
                    }
                }
                reranked
            }
            Err(e) => {
                tracing::warn!(
                    "Knowledge reranker failed ({}); falling back to hybrid ranking",
                    e
                );
                results.truncate(top_n);
                results
            }
        }
    }

    /// Search knowledge base with on-demand indexing
    pub async fn search(
        &self,
        query: &str,
        source: Option<&str>,
        session_id: Option<&str>,
        active_scope: Option<&str>,
    ) -> Result<Vec<KnowledgeSearchResult>> {
        // If source provided, normalize and check if needs indexing
        let normalized = source.map(normalize_source).transpose()?;
        let source_ref = normalized.as_deref();

        if let Some(s) = source_ref {
            if self.needs_indexing(s).await? {
                self.index_source_internal(s).await?;
            }
        }

        // Generate query embedding
        let query_embedding = crate::embedding::generate_embedding(
            query,
            self.embedding_provider.as_ref(),
            self.embedding_timeout_secs,
        )
        .await?;

        // Use global hybrid search flag
        let use_hybrid = self.search_config.hybrid.enabled;

        // Visible scope set: hot knowledge ("") + boxes bound to the active scope and
        // its ancestors. None = unscoped (no filter, all scopes) — mirrors memory.
        let scopes = crate::knowledge::boxes::visible_scopes(active_scope);

        // When reranking, pull a wider candidate set and let the cross-encoder
        // pick the final top-K; otherwise fetch exactly what we return. Empty
        // queries (filter-only) skip reranking — there's nothing to score.
        let rerank = self.search_config.reranker.enabled && !query.trim().is_empty();
        let candidate_limit = if rerank {
            self.search_config
                .reranker
                .top_k_candidates
                .max(self.config.max_results)
        } else {
            self.config.max_results
        };

        let results = self
            .store
            .search(
                &query_embedding,
                query,
                source_ref,
                candidate_limit,
                use_hybrid,
                session_id,
                scopes.as_deref(),
            )
            .await?;

        if rerank {
            Ok(self.rerank(query, results, self.config.max_results).await)
        } else {
            Ok(results)
        }
    }

    /// Check if source needs indexing (not indexed or outdated)
    async fn needs_indexing(&self, source: &str) -> Result<bool> {
        // stored:// and box:// content is managed explicitly (store command / box sync),
        // never auto-refetched as a URL or file.
        if source.starts_with("stored://")
            || source.starts_with(crate::knowledge::boxes::BOX_URI_PREFIX)
        {
            return Ok(false);
        }

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
                let doc = self
                    .chunker
                    .extract_and_chunk(&source, &content_type, &bytes)?;

                if doc.content_hash == content_hash {
                    return Ok(IndexResult {
                        source,
                        chunks_created: 0,
                        was_cached: true,
                        content_changed: false,
                    });
                }
            }
        }

        // Fetch and index
        let (content_type, bytes) = self.fetch_source(&source).await?;
        let doc = self
            .chunker
            .extract_and_chunk(&source, &content_type, &bytes)?;

        if doc.chunks.is_empty() {
            return Ok(IndexResult {
                source,
                chunks_created: 0,
                was_cached: false,
                content_changed: true,
            });
        }

        // Store (persistent hot knowledge — global scope, no session_id)
        let chunks_created = self
            .embed_and_store(&source, &doc.title, "", &doc.content_hash, doc.chunks, None)
            .await?;
        self.store.optimize().await;

        Ok(IndexResult {
            source,
            chunks_created,
            was_cached: false,
            content_changed: true,
        })
    }

    /// Internal indexing (always reindexes if outdated)
    async fn index_source_internal(&self, source: &str) -> Result<()> {
        let (content_type, bytes) = self.fetch_source(source).await?;
        let doc = self
            .chunker
            .extract_and_chunk(source, &content_type, &bytes)?;

        if doc.chunks.is_empty() {
            return Ok(());
        }

        self.embed_and_store(source, &doc.title, "", &doc.content_hash, doc.chunks, None)
            .await?;
        self.store.optimize().await;

        Ok(())
    }

    /// Fetch and return full text content of a source (URL or local file).
    /// This is a fallback for when search doesn't provide enough context.
    pub async fn read(&self, source: &str) -> Result<ReadResult> {
        let source = normalize_source(source)?;
        let (content_type, bytes) = self.fetch_source(&source).await?;
        let (title, content) = self.chunker.extract_text(&source, &content_type, &bytes)?;

        let content_type_str = match content_type {
            ContentType::Html => "html",
            ContentType::Markdown => "markdown",
            ContentType::PlainText => "text",
            ContentType::Pdf => "pdf",
            ContentType::Docx => "docx",
        };

        Ok(ReadResult {
            source,
            title,
            content,
            content_type: content_type_str.to_string(),
        })
    }

    /// Search indexed chunks by regex pattern, returning matching lines.
    /// Optionally filter by source and/or session.
    pub async fn match_content(
        &self,
        pattern: &str,
        source: Option<&str>,
        session_id: Option<&str>,
        active_scope: Option<&str>,
    ) -> Result<Vec<MatchResult>> {
        let regex = regex::Regex::new(pattern)
            .with_context(|| format!("Invalid regex pattern: {}", pattern))?;

        let scopes = crate::knowledge::boxes::visible_scopes(active_scope);
        self.store
            .match_content(&regex, source, session_id, scopes.as_deref())
            .await
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

    /// Store raw text content under a key, scoped to a session.
    /// Key must be unique within the session — returns error if it already exists.
    pub async fn store_content(
        &self,
        key: &str,
        content: &str,
        session_id: &str,
    ) -> Result<StoreResult> {
        let source = format!("stored://{}", key);

        // Check key uniqueness within session
        if self
            .store
            .has_source_in_session(&source, session_id)
            .await?
        {
            anyhow::bail!(
                "Key '{}' already exists in this session. Delete it first to replace.",
                key
            );
        }

        if content.trim().is_empty() {
            anyhow::bail!("Content cannot be empty");
        }

        let bytes = content.as_bytes();
        let doc = self
            .chunker
            .extract_and_chunk(&source, &ContentType::PlainText, bytes)?;

        // Content too small for the chunker to split — store it as one chunk.
        let chunks = if doc.chunks.is_empty() {
            vec![KnowledgeChunk {
                id: uuid::Uuid::new_v4().to_string(),
                source: source.clone(),
                source_title: doc.title.clone(),
                chunk_index: 0,
                content: content.to_string(),
                parent_content: None,
                section_path: vec![],
                char_start: 0,
                char_end: content.len(),
            }]
        } else {
            doc.chunks
        };

        let chunks_created = self
            .embed_and_store(
                &source,
                &doc.title,
                "",
                &doc.content_hash,
                chunks,
                Some(session_id),
            )
            .await?;
        self.store.optimize().await;

        Ok(StoreResult {
            source,
            chunks_created,
        })
    }

    /// Delete stored content by key within a session
    pub async fn delete_content(&self, key: &str, session_id: &str) -> Result<()> {
        let source = format!("stored://{}", key);
        self.store
            .delete_by_source_and_session(&source, session_id)
            .await
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

    // ========================================================================
    // Knowledge boxes
    // ========================================================================

    /// Manually import a remote git box and index it. Bound scope defaults to the
    /// org level derived from the repo (`host/org`); `--global` forces global ("")
    /// and `scope_override` pins an explicit scope.
    pub async fn import_box(
        &self,
        url: &str,
        scope_override: Option<&str>,
        global: bool,
    ) -> Result<usize> {
        use crate::knowledge::boxes;

        let box_id = boxes::box_id_from_url(url);
        let scope = if global {
            String::new()
        } else if let Some(s) = scope_override {
            s.to_string()
        } else {
            boxes::org_scope(&box_id).unwrap_or_default()
        };

        let dest = crate::storage::get_boxes_dir()?.join(boxes::slug(&box_id));
        if dest.exists() {
            boxes::pull(&dest).await.ok();
        } else {
            boxes::clone(url, &dest).await?;
        }

        let count = self.index_box_dir(&dest, &box_id, &scope).await?;

        let mut registry = boxes::BoxRegistry::load()?;
        registry.upsert(boxes::RemoteBox {
            url: url.to_string(),
            box_id: box_id.clone(),
            scope: scope.clone(),
            last_commit: boxes::head_commit(&dest).await.unwrap_or_default(),
            last_synced: Utc::now().to_rfc3339(),
        });
        registry.save()?;

        tracing::info!(
            "Imported box '{}' at scope '{}' ({} files indexed)",
            box_id,
            scope,
            count
        );
        Ok(count)
    }

    /// Bootstrap/refresh sync: index project `.box/` for the given repos, auto-probe
    /// each org for a conventional box, then pull + smart-reindex every subscribed
    /// remote box. Failures are logged and skipped, never fatal. `projects` is a list
    /// of (repo_path, scope) pairs discovered by the caller.
    pub async fn sync_boxes(&self, projects: &[(PathBuf, String)]) -> Result<()> {
        use crate::knowledge::boxes;
        let boxes_dir = crate::storage::get_boxes_dir()?;

        // 1. Project-local .box/ — box_id is the project scope itself.
        for (repo_path, scope) in projects {
            let box_dir = repo_path.join(boxes::PROJECT_BOX_DIR);
            if box_dir.is_dir() {
                if let Err(e) = self.index_box_dir(&box_dir, scope, scope).await {
                    tracing::warn!("Project box index failed ({}): {}", box_dir.display(), e);
                }
            }
        }

        let mut registry = boxes::BoxRegistry::load()?;
        let mut dirty = false;

        // 2. Org auto-probe by convention (<host>/<org>/octobrain-box).
        for (_repo, scope) in projects {
            let org = match boxes::org_scope(scope) {
                Some(o) => o,
                None => continue,
            };
            let box_id = boxes::org_box_id(&org);
            if registry.find(&box_id).is_some() {
                continue; // already subscribed — handled in the pull loop below
            }
            // Honour the negative-probe cache: skip orgs known to have no box.
            if registry
                .probed
                .get(&org)
                .map(|p| !p.has_box)
                .unwrap_or(false)
            {
                continue;
            }
            let url = boxes::org_box_url(&org);
            let exists = boxes::remote_exists(&url).await;
            registry.probed.insert(
                org.clone(),
                boxes::ProbeResult {
                    has_box: exists,
                    checked: Utc::now().to_rfc3339(),
                },
            );
            dirty = true;
            if exists {
                registry.upsert(boxes::RemoteBox {
                    url,
                    box_id,
                    scope: org,
                    last_commit: String::new(),
                    last_synced: String::new(),
                });
            }
        }

        // 3. Pull + smart-reindex every subscribed remote box.
        for b in registry.boxes.clone() {
            let dest = boxes_dir.join(boxes::slug(&b.box_id));
            if dest.exists() {
                boxes::pull(&dest).await.ok();
            } else if boxes::clone(&b.url, &dest).await.is_err() {
                continue; // unreachable/offline — keep whatever is already indexed
            }
            match self.index_box_dir(&dest, &b.box_id, &b.scope).await {
                Ok(_) => {
                    let last_commit = boxes::head_commit(&dest).await.unwrap_or_default();
                    if let Some(entry) = registry.boxes.iter_mut().find(|e| e.box_id == b.box_id) {
                        entry.last_commit = last_commit;
                        entry.last_synced = Utc::now().to_rfc3339();
                        dirty = true;
                    }
                }
                Err(e) => tracing::warn!("Box reindex failed ({}): {}", b.box_id, e),
            }
        }

        if dirty {
            registry.save()?;
        }
        Ok(())
    }

    /// Index a box directory: smart-reindex changed files (by content_hash), prune
    /// sources removed from the box. Returns the count of files (re)indexed this pass.
    async fn index_box_dir(&self, root: &Path, box_id: &str, scope: &str) -> Result<usize> {
        use crate::knowledge::boxes;

        let files = boxes::collect_indexable(root);
        let mut current: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut reindexed = 0usize;

        for (path, rel) in &files {
            let source = boxes::source_uri(box_id, rel);
            current.insert(source.clone());

            let content_type = ContentType::from_extension(&path.to_string_lossy())
                .unwrap_or(ContentType::PlainText);
            let bytes = match tokio::fs::read(path).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Skipping box file {}: {}", path.display(), e);
                    continue;
                }
            };

            let doc = self
                .chunker
                .extract_and_chunk(&source, &content_type, &bytes)?;
            if doc.chunks.is_empty() {
                continue;
            }

            // Smart reindex: skip files whose content is unchanged (embedding is the
            // expensive part; re-extraction/hashing is cheap).
            if let Some((existing_hash, _)) = self.store.get_source_metadata(&source).await? {
                if existing_hash == doc.content_hash {
                    continue;
                }
            }

            self.embed_and_store(
                &source,
                &doc.title,
                scope,
                &doc.content_hash,
                doc.chunks,
                None,
            )
            .await?;
            reindexed += 1;
        }

        // Prune sources removed from the box since the last sync.
        let prefix = boxes::source_prefix(box_id);
        let mut pruned = 0usize;
        for existing in self.store.list_sources_with_prefix(&prefix).await? {
            if !current.contains(&existing) {
                self.store.delete_source(&existing).await?;
                pruned += 1;
            }
        }

        // One compaction for the whole sweep; skip entirely on a no-op sync.
        if reindexed > 0 || pruned > 0 {
            self.store.optimize().await;
        }

        Ok(reindexed)
    }

    /// Remove a box: drop all its indexed rows, its registry entry, and its clone.
    pub async fn remove_box(&self, box_id: &str) -> Result<()> {
        use crate::knowledge::boxes;

        self.store
            .delete_by_source_prefix(&boxes::source_prefix(box_id))
            .await?;

        let mut registry = boxes::BoxRegistry::load()?;
        registry.remove(box_id);
        registry.save()?;

        let dest = crate::storage::get_boxes_dir()?.join(boxes::slug(box_id));
        if dest.exists() {
            std::fs::remove_dir_all(&dest).ok();
        }
        Ok(())
    }

    /// List subscribed remote boxes (project `.box/` boxes are discovered, not listed).
    pub fn list_boxes(&self) -> Result<Vec<crate::knowledge::boxes::RemoteBox>> {
        Ok(crate::knowledge::boxes::BoxRegistry::load()?.boxes)
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

    // Already a URL, stored key, or box source — pass through untouched.
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("stored://")
        || trimmed.starts_with("box://")
    {
        return Ok(trimmed.trim_end_matches('/').to_string());
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

    // Reject directories — knowledge sources must be a single file or URL.
    // Indexing a directory has no defined semantics; pass individual files instead.
    if canonical.is_dir() {
        anyhow::bail!(
            "Source must be a single file or URL, not a directory: {}. \
             Pass a specific file path (e.g. file.md, page.html) or an http(s):// URL.",
            canonical.display()
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_source_http_passthrough() {
        assert_eq!(
            normalize_source("https://example.com/page").unwrap(),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_source("http://example.com").unwrap(),
            "http://example.com"
        );
    }

    #[test]
    fn test_normalize_source_stored_passthrough() {
        assert_eq!(
            normalize_source("stored://my_key").unwrap(),
            "stored://my_key"
        );
        assert_eq!(
            normalize_source("stored://web_results").unwrap(),
            "stored://web_results"
        );
    }

    #[test]
    fn test_normalize_source_file_uri_passthrough() {
        assert_eq!(
            normalize_source("file:///tmp/test.txt").unwrap(),
            "file:///tmp/test.txt"
        );
    }

    #[test]
    fn test_normalize_source_trims_whitespace() {
        assert_eq!(
            normalize_source("  https://example.com  ").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_source("  stored://key  ").unwrap(),
            "stored://key"
        );
    }

    #[test]
    fn test_is_local_source() {
        assert!(is_local_source("file:///tmp/test.txt"));
        assert!(is_local_source("/absolute/path"));
        assert!(!is_local_source("https://example.com"));
        assert!(!is_local_source("stored://key"));
        assert!(!is_local_source("http://example.com"));
    }

    #[test]
    fn test_source_to_path() {
        assert_eq!(
            source_to_path("file:///tmp/test.txt").unwrap(),
            PathBuf::from("/tmp/test.txt")
        );
        assert_eq!(
            source_to_path("/absolute/path").unwrap(),
            PathBuf::from("/absolute/path")
        );
        assert!(source_to_path("https://example.com").is_err());
        assert!(source_to_path("stored://key").is_err());
    }

    #[test]
    fn test_normalize_source_rejects_directory() {
        // Use the platform temp dir — guaranteed to exist on every OS (incl. Windows).
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_str().expect("temp dir path must be valid UTF-8");
        let err = normalize_source(tmp_str)
            .expect_err("directories must not be accepted as knowledge sources");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("not a directory") || msg.contains("single file or URL"),
            "error should mention directory rejection, got: {msg}"
        );
    }
}
