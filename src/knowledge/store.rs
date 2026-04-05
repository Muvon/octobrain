// Copyright 2026 Muvon Un Limited
//
use anyhow::{Context, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int32Array, ListArray, RecordBatch, StringArray,
    TimestampMillisecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::{
    connect,
    index::Index,
    query::{ExecutableQuery, QueryBase, QueryExecutionOptions},
    table::OptimizeAction,
    Connection, DistanceType, Table,
};
use std::sync::Arc;

use crate::knowledge::types::{KnowledgeChunk, KnowledgeSearchResult, KnowledgeStats};
use chrono::Duration;

/// RRF (Reciprocal Rank Fusion) constant k
/// Default value from LanceDB, based on research paper:
/// https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf
/// "Experiments indicate that k = 60 was near-optimal, but that the choice is not critical"
const RRF_K: f32 = 60.0;

pub struct KnowledgeStore {
    table: Table,
    schema: Arc<Schema>,
    vector_dim: usize,
}

impl KnowledgeStore {
    fn quote_filter_string(input: &str) -> String {
        input.replace('\'', "''")
    }

    pub async fn new(vector_dim: usize) -> Result<Self> {
        let db_path = crate::storage::get_system_storage_dir()?.join("knowledge");
        std::fs::create_dir_all(&db_path)?;

        let db = connect(db_path.to_str().unwrap()).execute().await?;
        let schema = Self::build_schema(vector_dim);

        Self::initialize_table(&db, &schema).await?;

        // Cache the table handle — opened once, reused for the lifetime of this store
        let table = db.open_table("knowledge_chunks").execute().await?;

        Ok(Self {
            table,
            schema,
            vector_dim,
        })
    }

    fn build_schema(vector_dim: usize) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("source_title", DataType::Utf8, false),
            Field::new("session_id", DataType::Utf8, true),
            Field::new("chunk_index", DataType::Int32, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("parent_content", DataType::Utf8, false),
            Field::new(
                "section_path",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new("char_start", DataType::Int32, false),
            Field::new("char_end", DataType::Int32, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new(
                "indexed_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new(
                "last_checked",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vector_dim as i32,
                ),
                false,
            ),
        ]))
    }

    async fn initialize_table(db: &Connection, schema: &Arc<Schema>) -> Result<()> {
        let table_names = db.table_names().execute().await?;

        // Drop table if schema is outdated (missing columns)
        if table_names.contains(&"knowledge_chunks".to_string()) {
            let table = db.open_table("knowledge_chunks").execute().await?;
            let existing_schema = table.schema().await?;
            let needs_recreate = schema
                .fields()
                .iter()
                .any(|f| existing_schema.field_with_name(f.name()).is_err());
            if needs_recreate {
                tracing::info!("knowledge_chunks schema outdated, dropping and recreating");
                db.drop_table("knowledge_chunks", &[]).await?;
            } else {
                return Ok(());
            }
        }

        // Create table with empty batch (schema-only creation)
        use arrow::record_batch::RecordBatchIterator;
        use std::iter::once;
        let empty_batch = RecordBatch::new_empty(schema.clone());
        let batch_reader = RecordBatchIterator::new(once(Ok(empty_batch)), schema.clone());
        db.create_table("knowledge_chunks", batch_reader)
            .execute()
            .await?;

        // Create FTS index on content column for hybrid search (BM25 + Vector)
        let table = db.open_table("knowledge_chunks").execute().await?;
        table
            .create_index(&["content"], Index::FTS(Default::default()))
            .execute()
            .await
            .context("Failed to create FTS index on content column")?;

        tracing::info!("Created FTS index on knowledge_chunks.content for hybrid search");

        Ok(())
    }

    pub async fn store_chunks(
        &self,
        source: &str,
        source_title: &str,
        content_hash: &str,
        chunks: &[KnowledgeChunk],
        embeddings: &[Vec<f32>],
        session_id: Option<&str>,
    ) -> Result<()> {
        // Delete existing chunks: session-scoped deletes only within session,
        // persistent deletes all chunks for source (full reindex)
        if let Some(sid) = session_id {
            self.delete_by_source_and_session(source, sid).await?;
        } else {
            self.delete_source(source).await?;
        }

        if chunks.is_empty() {
            return Ok(());
        }

        let now = Utc::now();
        let now_millis = now.timestamp_millis();

        // Build arrays
        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let sources: Vec<&str> = chunks.iter().map(|_| source).collect();
        let source_titles: Vec<&str> = chunks.iter().map(|_| source_title).collect();
        let session_ids: Vec<Option<&str>> = chunks.iter().map(|_| session_id).collect();
        let chunk_indices: Vec<i32> = chunks.iter().map(|c| c.chunk_index).collect();
        let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let parent_contents: Vec<&str> = chunks
            .iter()
            .map(|c| c.parent_content.as_deref().unwrap_or(""))
            .collect();
        let char_starts: Vec<i32> = chunks.iter().map(|c| c.char_start as i32).collect();
        let char_ends: Vec<i32> = chunks.iter().map(|c| c.char_end as i32).collect();
        let content_hashes: Vec<&str> = chunks.iter().map(|_| content_hash).collect();
        let indexed_ats: Vec<i64> = chunks.iter().map(|_| now_millis).collect();
        let last_checkeds: Vec<i64> = chunks.iter().map(|_| now_millis).collect();

        // Build section_path list array
        let mut section_path_builder =
            arrow_array::builder::ListBuilder::new(arrow_array::builder::StringBuilder::new());
        for chunk in chunks {
            for section in &chunk.section_path {
                section_path_builder.values().append_value(section);
            }
            section_path_builder.append(true);
        }
        let section_path_array = section_path_builder.finish();

        // Build embedding array
        let embedding_values: Vec<f32> =
            embeddings.iter().flat_map(|e| e.iter().copied()).collect();
        let embedding_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            self.vector_dim as i32,
            Arc::new(Float32Array::from(embedding_values)),
            None,
        )?;

        let batch = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(sources)),
                Arc::new(StringArray::from(source_titles)),
                Arc::new(StringArray::from(session_ids)),
                Arc::new(Int32Array::from(chunk_indices)),
                Arc::new(StringArray::from(contents)),
                Arc::new(StringArray::from(parent_contents)),
                Arc::new(section_path_array),
                Arc::new(Int32Array::from(char_starts)),
                Arc::new(Int32Array::from(char_ends)),
                Arc::new(StringArray::from(content_hashes)),
                Arc::new(TimestampMillisecondArray::from(indexed_ats)),
                Arc::new(TimestampMillisecondArray::from(last_checkeds)),
                Arc::new(embedding_array),
            ],
        )?;

        use arrow::record_batch::RecordBatchIterator;
        use std::iter::once;
        let batch_reader = RecordBatchIterator::new(once(Ok(batch)), self.schema.clone());
        self.table.add(batch_reader).execute().await?;

        // Compact fragments and update FTS/scalar indexes so new chunks are immediately
        // searchable without brute-force fallback on the unindexed portion.
        self.table.optimize(OptimizeAction::All).await.ok();

        Ok(())
    }

    pub async fn search(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        source: Option<&str>,
        limit: usize,
        use_hybrid: bool,
        session_id: Option<&str>,
    ) -> Result<Vec<KnowledgeSearchResult>> {
        let mut query = self
            .table
            .vector_search(query_embedding)?
            .distance_type(DistanceType::Cosine)
            .limit(limit);

        // Add full-text search for hybrid mode
        if use_hybrid {
            let fts_query = FullTextSearchQuery::new(query_text.to_string());
            query = query.full_text_search(fts_query);
        }

        // Build filter conditions
        let mut filters = Vec::new();

        if let Some(s) = source {
            filters.push(format!("source = '{}'", Self::quote_filter_string(s)));
        }

        // Session scoping: return persistent (NULL session_id) + current session's data
        if let Some(sid) = session_id {
            filters.push(format!(
                "(session_id IS NULL OR session_id = '{}')",
                Self::quote_filter_string(sid)
            ));
        }

        if !filters.is_empty() {
            query = query.only_if(filters.join(" AND "));
        }

        // Execute hybrid search if enabled, otherwise regular vector search
        let mut results = if use_hybrid {
            query
                .execute_hybrid(QueryExecutionOptions::default())
                .await?
        } else {
            query.execute().await?
        };

        let mut search_results = Vec::new();

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }

            let ids = batch
                .column_by_name("id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let sources = batch
                .column_by_name("source")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let source_titles = batch
                .column_by_name("source_title")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let session_ids = batch
                .column_by_name("session_id")
                .and_then(|col| col.as_any().downcast_ref::<StringArray>());
            let chunk_indices = batch
                .column_by_name("chunk_index")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            let contents = batch
                .column_by_name("content")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let parent_contents = batch
                .column_by_name("parent_content")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let section_paths = batch
                .column_by_name("section_path")
                .unwrap()
                .as_any()
                .downcast_ref::<ListArray>()
                .unwrap();
            let char_starts = batch
                .column_by_name("char_start")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            let char_ends = batch
                .column_by_name("char_end")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            // Extract score column - hybrid search uses _relevance_score, vector search uses _distance
            // LanceDB hybrid search with RRF reranking returns _relevance_score (raw RRF scores)
            // RRF formula: score = sum of 1/(rank + k) for each ranking (vector + FTS)
            // Max possible score is 2/k (if rank 0 in both)
            // Regular vector search returns _distance (0-2 for cosine, lower is better)
            let relevance_scores: Vec<f32> = if use_hybrid {
                // Hybrid search: normalize RRF scores to 0-1 range
                // Max possible RRF score is 2/k (when rank=0 in both vector and FTS)
                let max_rrf_score = 2.0 / RRF_K;

                batch
                    .column_by_name("_relevance_score")
                    .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
                    .map(|arr| {
                        (0..arr.len())
                            .map(|i| {
                                let raw_score = arr.value(i);
                                // Normalize: divide by max possible score
                                (raw_score / max_rrf_score).min(1.0)
                            })
                            .collect::<Vec<f32>>()
                    })
                    .unwrap_or_else(|| vec![0.5; batch.num_rows()])
            } else {
                // Vector search: convert _distance to relevance (1.0 - distance for cosine)
                batch
                    .column_by_name("_distance")
                    .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
                    .map(|arr| {
                        (0..arr.len())
                            .map(|i| 1.0 - arr.value(i))
                            .collect::<Vec<f32>>()
                    })
                    .unwrap_or_else(|| vec![0.5; batch.num_rows()])
            };

            for (i, &relevance_score) in relevance_scores.iter().enumerate() {
                let section_path_array = section_paths.value(i);
                let section_path_strings = section_path_array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let section_path: Vec<String> = (0..section_path_strings.len())
                    .map(|j| section_path_strings.value(j).to_string())
                    .collect();

                let is_session_scoped = session_ids
                    .map(|arr| !arr.is_null(i) && !arr.value(i).is_empty())
                    .unwrap_or(false);

                let chunk = KnowledgeChunk {
                    id: ids.value(i).to_string(),
                    source: sources.value(i).to_string(),
                    source_title: source_titles.value(i).to_string(),
                    chunk_index: chunk_indices.value(i),
                    content: contents.value(i).to_string(),
                    parent_content: {
                        let p = parent_contents.value(i);
                        if p.is_empty() {
                            None
                        } else {
                            Some(p.to_string())
                        }
                    },
                    section_path,
                    char_start: char_starts.value(i) as usize,
                    char_end: char_ends.value(i) as usize,
                };

                search_results.push(KnowledgeSearchResult {
                    chunk,
                    relevance_score,
                    session_scoped: is_session_scoped,
                });
            }
        }

        Ok(search_results)
    }

    pub async fn get_source_metadata(
        &self,
        source: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>> {
        let query = self
            .table
            .query()
            .only_if(format!("source = '{}'", Self::quote_filter_string(source)))
            .limit(1);

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }

        let batch = &batches[0];
        let content_hashes = batch
            .column_by_name("content_hash")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let last_checkeds = batch
            .column_by_name("last_checked")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .unwrap();

        let content_hash = content_hashes.value(0).to_string();
        let last_checked_millis = last_checkeds.value(0);
        let last_checked =
            DateTime::from_timestamp_millis(last_checked_millis).context("Invalid timestamp")?;

        Ok(Some((content_hash, last_checked)))
    }

    pub async fn delete_source(&self, source: &str) -> Result<()> {
        self.table
            .delete(&format!("source = '{}'", Self::quote_filter_string(source)))
            .await?;
        Ok(())
    }

    pub async fn get_stats(&self) -> Result<KnowledgeStats> {
        let count = self.table.count_rows(None).await?;

        if count == 0 {
            return Ok(KnowledgeStats {
                total_sources: 0,
                total_chunks: 0,
                oldest_indexed: None,
                newest_indexed: None,
            });
        }
        // Get all data to compute stats
        let results = self.table.query().execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut unique_urls = std::collections::HashSet::new();
        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        for batch in batches {
            let sources_col = batch
                .column_by_name("source")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let indexed_ats = batch
                .column_by_name("indexed_at")
                .unwrap()
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .unwrap();

            for i in 0..batch.num_rows() {
                unique_urls.insert(sources_col.value(i).to_string());

                let indexed_millis = indexed_ats.value(i);
                if let Some(indexed) = DateTime::from_timestamp_millis(indexed_millis) {
                    if oldest.is_none_or(|old| indexed < old) {
                        oldest = Some(indexed);
                    }
                    if newest.is_none_or(|new| indexed > new) {
                        newest = Some(indexed);
                    }
                }
            }
        }

        Ok(KnowledgeStats {
            total_sources: unique_urls.len(),
            total_chunks: count,
            oldest_indexed: oldest,
            newest_indexed: newest,
        })
    }

    pub async fn list_sources(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, usize, DateTime<Utc>)>> {
        let results = self.table.query().execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut sources: std::collections::HashMap<String, (String, usize, DateTime<Utc>)> =
            std::collections::HashMap::new();

        for batch in batches {
            let sources_col = batch
                .column_by_name("source")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let source_titles = batch
                .column_by_name("source_title")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let last_checkeds = batch
                .column_by_name("last_checked")
                .unwrap()
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .unwrap();

            for i in 0..batch.num_rows() {
                let url = sources_col.value(i).to_string();
                let title = source_titles.value(i).to_string();
                let last_checked_millis = last_checkeds.value(i);
                let last_checked = DateTime::from_timestamp_millis(last_checked_millis)
                    .context("Invalid timestamp")?;

                sources
                    .entry(url.clone())
                    .and_modify(|(_, count, existing_last_checked)| {
                        *count += 1;
                        if last_checked > *existing_last_checked {
                            *existing_last_checked = last_checked;
                        }
                    })
                    .or_insert((title, 1, last_checked));
            }
        }

        let mut result: Vec<(String, String, usize, DateTime<Utc>)> = sources
            .into_iter()
            .map(|(url, (title, count, last_checked))| (url, title, count, last_checked))
            .collect();

        // Sort by last_checked descending
        result.sort_by(|a, b| b.3.cmp(&a.3));

        if let Some(limit) = limit {
            result.truncate(limit);
        }

        Ok(result)
    }

    /// Check if a source exists for a given session
    pub async fn has_source_in_session(&self, source: &str, session_id: &str) -> Result<bool> {
        let query = self
            .table
            .query()
            .only_if(format!(
                "source = '{}' AND session_id = '{}'",
                Self::quote_filter_string(source),
                Self::quote_filter_string(session_id)
            ))
            .limit(1);

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;
        Ok(!batches.is_empty() && batches[0].num_rows() > 0)
    }

    /// Delete stored content by source and session
    pub async fn delete_by_source_and_session(&self, source: &str, session_id: &str) -> Result<()> {
        self.table
            .delete(&format!(
                "source = '{}' AND session_id = '{}'",
                Self::quote_filter_string(source),
                Self::quote_filter_string(session_id)
            ))
            .await?;
        Ok(())
    }

    /// Clean up expired session-scoped chunks (crash recovery)
    pub async fn cleanup_expired_sessions(&self, ttl_hours: u64) -> Result<()> {
        let cutoff = Utc::now() - Duration::hours(ttl_hours as i64);
        let cutoff_millis = cutoff.timestamp_millis();
        self.table
            .delete(&format!(
                "session_id IS NOT NULL AND indexed_at < {}",
                cutoff_millis
            ))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a store with a unique temp directory
    async fn test_store(vector_dim: usize) -> KnowledgeStore {
        let db_path = std::env::temp_dir().join(format!("octobrain_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&db_path).unwrap();

        let db = connect(db_path.to_str().unwrap()).execute().await.unwrap();
        let schema = KnowledgeStore::build_schema(vector_dim);
        KnowledgeStore::initialize_table(&db, &schema)
            .await
            .unwrap();
        let table = db.open_table("knowledge_chunks").execute().await.unwrap();

        KnowledgeStore {
            table,
            schema,
            vector_dim,
        }
    }

    fn make_chunk(id: &str, source: &str, content: &str) -> KnowledgeChunk {
        KnowledgeChunk {
            id: id.to_string(),
            source: source.to_string(),
            source_title: "Test".to_string(),
            chunk_index: 0,
            content: content.to_string(),
            parent_content: None,
            section_path: vec![],
            char_start: 0,
            char_end: content.len(),
        }
    }

    fn dummy_embedding(dim: usize) -> Vec<f32> {
        vec![0.1; dim]
    }

    #[tokio::test]
    async fn test_store_and_search_persistent() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "https://example.com", "hello world test content");
        let embedding = dummy_embedding(dim);

        store
            .store_chunks(
                "https://example.com",
                "Example",
                "hash1",
                &[chunk],
                &[embedding.clone()],
                None,
            )
            .await
            .unwrap();

        // Search without session filter — should find persistent content
        let results = store
            .search(&embedding, "hello", None, 10, false, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].session_scoped);
        assert_eq!(results[0].chunk.source, "https://example.com");
    }

    #[tokio::test]
    async fn test_store_and_search_session_scoped() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "stored://my_key", "session specific content");
        let embedding = dummy_embedding(dim);

        store
            .store_chunks(
                "stored://my_key",
                "My Key",
                "hash1",
                &[chunk],
                &[embedding.clone()],
                Some("session-abc"),
            )
            .await
            .unwrap();

        // Search with matching session — should find it
        let results = store
            .search(&embedding, "session", None, 10, false, Some("session-abc"))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].session_scoped);
        assert_eq!(results[0].chunk.source, "stored://my_key");
    }

    #[tokio::test]
    async fn test_session_isolation() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "stored://secret", "session A data");
        let embedding = dummy_embedding(dim);

        // Store with session A
        store
            .store_chunks(
                "stored://secret",
                "Secret",
                "hash1",
                &[chunk],
                &[embedding.clone()],
                Some("session-A"),
            )
            .await
            .unwrap();

        // Search with session B — should NOT find session A's data
        let results = store
            .search(&embedding, "secret", None, 10, false, Some("session-B"))
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_persistent_visible_to_all_sessions() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "https://docs.rs", "persistent docs");
        let embedding = dummy_embedding(dim);

        // Store persistent (no session)
        store
            .store_chunks(
                "https://docs.rs",
                "Docs",
                "hash1",
                &[chunk],
                &[embedding.clone()],
                None,
            )
            .await
            .unwrap();

        // Search with any session — should find persistent
        let results = store
            .search(&embedding, "docs", None, 10, false, Some("any-session"))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].session_scoped);
    }

    #[tokio::test]
    async fn test_has_source_in_session() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "stored://key1", "content");
        let embedding = dummy_embedding(dim);

        store
            .store_chunks(
                "stored://key1",
                "Key1",
                "hash1",
                &[chunk],
                &[embedding],
                Some("sess1"),
            )
            .await
            .unwrap();

        assert!(store
            .has_source_in_session("stored://key1", "sess1")
            .await
            .unwrap());
        assert!(!store
            .has_source_in_session("stored://key1", "sess2")
            .await
            .unwrap());
        assert!(!store
            .has_source_in_session("stored://key2", "sess1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_delete_by_source_and_session() {
        let dim = 4;
        let store = test_store(dim).await;
        let chunk = make_chunk("c1", "stored://key1", "content to delete");
        let embedding = dummy_embedding(dim);

        store
            .store_chunks(
                "stored://key1",
                "Key1",
                "hash1",
                &[chunk],
                &[embedding.clone()],
                Some("sess1"),
            )
            .await
            .unwrap();

        assert!(store
            .has_source_in_session("stored://key1", "sess1")
            .await
            .unwrap());

        store
            .delete_by_source_and_session("stored://key1", "sess1")
            .await
            .unwrap();

        assert!(!store
            .has_source_in_session("stored://key1", "sess1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_mixed_persistent_and_session_search() {
        let dim = 4;
        let store = test_store(dim).await;
        let embedding = dummy_embedding(dim);

        // Store persistent chunk
        let persistent = make_chunk("p1", "https://example.com", "persistent data");
        store
            .store_chunks(
                "https://example.com",
                "Example",
                "hash1",
                &[persistent],
                &[embedding.clone()],
                None,
            )
            .await
            .unwrap();

        // Store session chunk
        let session = make_chunk("s1", "stored://notes", "session data");
        store
            .store_chunks(
                "stored://notes",
                "Notes",
                "hash2",
                &[session],
                &[embedding.clone()],
                Some("sess1"),
            )
            .await
            .unwrap();

        // Search with matching session — should see both
        let results = store
            .search(&embedding, "data", None, 10, false, Some("sess1"))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);

        let session_count = results.iter().filter(|r| r.session_scoped).count();
        let persistent_count = results.iter().filter(|r| !r.session_scoped).count();
        assert_eq!(session_count, 1);
        assert_eq!(persistent_count, 1);
    }
}
