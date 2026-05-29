// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

// Arrow imports
use arrow_array::{Array, FixedSizeListArray, Float32Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};

// LanceDB imports
use futures::TryStreamExt;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::{
    connect,
    index::Index,
    query::{ExecutableQuery, QueryBase, QueryExecutionOptions},
    table::{NewColumnTransform, OptimizeAction},
    Connection, DistanceType, Table,
};

/// RRF (Reciprocal Rank Fusion) constant k — same value used by knowledge store.
/// Based on: https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf
/// "Experiments indicate that k = 60 was near-optimal"
const RRF_K: f32 = 60.0;

/// Rocchio query expansion: `alpha * query + (1 - alpha) * centroid`, then L2-normalized.
///
/// Pure-math helper extracted so it can be unit-tested without LanceDB. `alpha` is clamped
/// to [0.0, 1.0]. The output is normalized so cosine similarity over the blended vector
/// matches the geometry the rest of the search path expects.
pub(crate) fn rocchio_blend(query: &[f32], centroid: &[f32], alpha: f32) -> Vec<f32> {
    debug_assert_eq!(query.len(), centroid.len());
    let alpha = alpha.clamp(0.0, 1.0);
    let mut blended: Vec<f32> = query
        .iter()
        .zip(centroid.iter())
        .map(|(q, c)| alpha * q + (1.0 - alpha) * c)
        .collect();

    let norm: f32 = blended.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        let inv = 1.0 / norm;
        for v in blended.iter_mut() {
            *v *= inv;
        }
    }
    blended
}

use super::reranker_integration::RerankerIntegration;
use super::types::{Memory, MemoryConfig, MemoryQuery, MemoryRelationship, MemorySearchResult};
use crate::arrow_helpers::{
    f32_column, f32_column_opt, i32_column_opt, string_column, string_column_opt,
};
use crate::embedding::EmbeddingProvider;

/// Escape a string for safe inclusion inside a LanceDB SQL single-quoted literal.
/// DataFusion (LanceDB's SQL engine) escapes an embedded `'` by doubling it.
fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

/// Build a SQL predicate string for scalar fields that LanceDB can filter at the storage layer.
///
/// Tags and related_files are excluded here because they are stored as JSON-serialized strings
/// and cannot be queried with simple SQL equality — those are handled post-fetch in Rust.
fn build_scalar_predicate(
    project_key: Option<&str>,
    role: Option<&str>,
    query: &MemoryQuery,
) -> String {
    // project_key is optional — None means no project filter (show all projects)
    let mut parts: Vec<String> = if let Some(key) = project_key {
        vec![format!("project_key = '{}'", escape_sql(key))]
    } else {
        Vec::new()
    };

    // role filter — only applied when a role is set (None = no filter)
    if let Some(role) = role {
        parts.push(format!("role = '{}'", escape_sql(role)));
    }

    if let Some(ref memory_types) = query.memory_types {
        if !memory_types.is_empty() {
            let list = memory_types
                .iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("memory_type IN ({})", list));
        }
    }

    if let Some(min_importance) = query.min_importance {
        parts.push(format!("importance >= {}", min_importance));
    }

    if let Some(min_confidence) = query.min_confidence {
        parts.push(format!("confidence >= {}", min_confidence));
    }

    if let Some(ref git_commit) = query.git_commit {
        // Escape defensively — commit hashes won't contain quotes, but stay consistent.
        parts.push(format!("git_commit = '{}'", escape_sql(git_commit)));
    }

    if let Some(created_after) = query.created_after {
        parts.push(format!("created_at >= '{}'", created_after.to_rfc3339()));
    }

    if let Some(created_before) = query.created_before {
        parts.push(format!("created_at <= '{}'", created_before.to_rfc3339()));
    }

    parts.join(" AND ")
}

/// LanceDB-based storage for memories with vector search capabilities
pub struct MemoryStore {
    memories_table: Table,
    relationships_table: Table,
    schema: Arc<Schema>,
    rel_schema: Arc<Schema>,
    embedding_provider: Box<dyn EmbeddingProvider>,
    config: MemoryConfig,
    main_config: crate::config::Config,
    vector_dim: usize,
    // Wrapped in a std::sync::Mutex so enable/disable_reranker can mutate it
    // via `&self`, which lets the whole `MemoryStore` be shared as `Arc<Self>`
    // — needed for fire-and-forget auto-linking from `MemoryManager::memorize`.
    // The lock is only held for the brief enable/disable mutation and during
    // the rerank call itself; never held across awaits inside async work.
    reranker_integration: std::sync::Mutex<Option<RerankerIntegration>>,
    project_key: Option<String>,
    role: Option<String>,
}

impl MemoryStore {
    /// Returns true when no project key is set (global/unscoped context).
    pub fn has_no_project_key(&self) -> bool {
        self.project_key.is_none()
    }

    /// Arrow schema for the `memories` table. Defined once so the writer
    /// (`store_memory_with_embedding`) and the table creator (`init_tables`)
    /// can never drift out of sync.
    fn memories_schema(vector_dim: usize) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("project_key", DataType::Utf8, false),
            Field::new("role", DataType::Utf8, true),
            Field::new("memory_type", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
            Field::new("importance", DataType::Float32, false),
            Field::new("confidence", DataType::Float32, false),
            Field::new("tags", DataType::Utf8, true),
            Field::new("related_files", DataType::Utf8, true),
            Field::new("git_commit", DataType::Utf8, true),
            Field::new("source", DataType::Utf8, false),
            // Decay state, persisted so retrieval ranking actually differentiates memories.
            // Int32 (not UInt32) so the migration SQL `CAST(0 AS INT)` is portable across
            // DataFusion SQL-parser versions, and to match what the writer produces below.
            Field::new("access_count", DataType::Int32, false),
            Field::new("last_accessed", DataType::Utf8, false),
            // Lifecycle state for goal-anchored consolidation. Stores `MemoryState`
            // as a lowercase string ("working" | "consolidated" | "archived").
            Field::new("state", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vector_dim as i32,
                ),
                true,
            ),
        ]))
    }

    /// Arrow schema for the `memory_relationships` table.
    fn relationships_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, false),
            Field::new("target_id", DataType::Utf8, false),
            Field::new("project_key", DataType::Utf8, false),
            Field::new("relationship_type", DataType::Utf8, false),
            Field::new("strength", DataType::Float32, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
        ]))
    }

    /// project_key used for writes/deletes, falling back to "default" when the
    /// store is unscoped. Centralizes the repeated `unwrap_or("default")`.
    fn project_label(&self) -> &str {
        self.project_key.as_deref().unwrap_or("default")
    }

    /// Current importance for `memory` under this store's decay configuration.
    /// Wraps the four-argument decay plumbing repeated across the search paths.
    fn current_importance(&self, memory: &Memory) -> f32 {
        memory.get_current_importance(
            self.config.decay_enabled,
            self.config.min_importance_threshold,
            self.config.decay_half_life_days,
            self.config.access_boost_factor,
        )
    }

    /// Build the IVF_PQ vector index on the `embedding` column from optimizer params.
    async fn create_vector_index(
        &self,
        params: crate::vector_optimizer::IndexParams,
    ) -> Result<()> {
        self.memories_table
            .create_index(
                &["embedding"],
                Index::IvfPq(
                    lancedb::index::vector::IvfPqIndexBuilder::default()
                        .distance_type(params.distance_type)
                        .num_partitions(params.num_partitions)
                        .num_sub_vectors(params.num_sub_vectors)
                        .num_bits(params.num_bits as u32),
                ),
            )
            .execute()
            .await?;
        Ok(())
    }

    /// Create a new memory store
    pub async fn new(
        db_path: &str,
        project_key: Option<String>,
        role: Option<String>,
        embedding_provider: Box<dyn EmbeddingProvider>,
        config: MemoryConfig,
        main_config: crate::config::Config,
        reranker_integration: Option<RerankerIntegration>,
    ) -> Result<Self> {
        let reranker_integration = std::sync::Mutex::new(reranker_integration);
        let db = connect(db_path).execute().await?;

        // Get vector dimension from the embedding provider by testing with a short text
        let test_embedding = embedding_provider.generate_embedding("test").await?;
        let vector_dim = test_embedding.len();

        // Build the memories schema once — reused for every write
        let schema = Self::memories_schema(vector_dim);

        // Initialize tables (creates them if missing, adds scalar + FTS indexes)
        Self::init_tables(&db, &schema).await?;

        // Cache table handles — opened once, reused for the lifetime of this store
        let memories_table = db.open_table("memories").execute().await?;
        let relationships_table = db.open_table("memory_relationships").execute().await?;

        // Migrate existing tables that pre-date the access_count / last_accessed columns.
        // New tables created above already have them; this only adds them where missing.
        Self::migrate_decay_columns(&memories_table).await?;
        Self::migrate_state_column(&memories_table).await?;

        // Build relationship schema once — reused for every relationship write
        let rel_schema = Self::relationships_schema();

        let store = Self {
            memories_table,
            relationships_table,
            schema,
            rel_schema,
            embedding_provider,
            config,
            main_config,
            vector_dim,
            reranker_integration,
            project_key,
            role,
        };
        // Ensure optimal vector index (only during initialization, not on every store)
        store.ensure_optimal_index().await?;

        Ok(store)
    }

    /// Add `access_count` and `last_accessed` columns to pre-existing memory tables that
    /// were created before the decay-persistence change. New tables already have them
    /// via the schema in `new()`. Defaults: access_count=0, last_accessed=created_at.
    async fn migrate_decay_columns(table: &Table) -> Result<()> {
        let schema = table.schema().await?;
        let has_access_count = schema.field_with_name("access_count").is_ok();
        let has_last_accessed = schema.field_with_name("last_accessed").is_ok();

        let mut transforms: Vec<(String, String)> = Vec::new();
        if !has_access_count {
            transforms.push(("access_count".to_string(), "CAST(0 AS INT)".to_string()));
        }
        if !has_last_accessed {
            transforms.push(("last_accessed".to_string(), "created_at".to_string()));
        }

        if transforms.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "Migrating memories table: adding {} decay column(s)",
            transforms.len()
        );
        table
            .add_columns(NewColumnTransform::SqlExpressions(transforms), None)
            .await
            .context("Failed to add decay columns to existing memories table")?;
        Ok(())
    }

    /// Add the `state` column to pre-existing memory tables created before the
    /// lifecycle-state / goal-consolidation change. Default value is `'working'`
    /// so all legacy memories remain fully active.
    async fn migrate_state_column(table: &Table) -> Result<()> {
        let schema = table.schema().await?;
        if schema.field_with_name("state").is_ok() {
            return Ok(());
        }
        tracing::info!("Migrating memories table: adding 'state' column");
        table
            .add_columns(
                NewColumnTransform::SqlExpressions(vec![(
                    "state".to_string(),
                    "'working'".to_string(),
                )]),
                None,
            )
            .await
            .context("Failed to add state column to existing memories table")?;
        Ok(())
    }

    /// Initialize memory and relationship tables (static — called once from new())
    async fn init_tables(db: &Connection, schema: &Arc<Schema>) -> Result<()> {
        let table_names = db.table_names().execute().await?;

        // Create memories table if it doesn't exist
        if !table_names.contains(&"memories".to_string()) {
            db.create_empty_table("memories", schema.clone())
                .execute()
                .await?;

            let table = db.open_table("memories").execute().await?;

            // Scalar indexes for pushdown filtering — created once at table birth
            // Bitmap: low-cardinality string columns (project_key, memory_type, source)
            table
                .create_index(&["project_key"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memories.project_key")?;
            table
                .create_index(&["memory_type"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memories.memory_type")?;
            table
                .create_index(&["source"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memories.source")?;
            table
                .create_index(&["role"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memories.role")?;

            // BTree: range-query columns (importance, confidence, created_at)
            table
                .create_index(&["importance"], Index::BTree(Default::default()))
                .execute()
                .await
                .context("Failed to create BTree index on memories.importance")?;
            table
                .create_index(&["confidence"], Index::BTree(Default::default()))
                .execute()
                .await
                .context("Failed to create BTree index on memories.confidence")?;
            table
                .create_index(&["created_at"], Index::BTree(Default::default()))
                .execute()
                .await
                .context("Failed to create BTree index on memories.created_at")?;

            // FTS indexes for native BM25 hybrid search
            table
                .create_index(&["content"], Index::FTS(Default::default()))
                .execute()
                .await
                .context("Failed to create FTS index on memories.content")?;
            table
                .create_index(&["title"], Index::FTS(Default::default()))
                .execute()
                .await
                .context("Failed to create FTS index on memories.title")?;

            tracing::info!("Created scalar (Bitmap/BTree) and FTS indexes on memories table");
        }

        // Create relationships table if it doesn't exist
        if !table_names.contains(&"memory_relationships".to_string()) {
            db.create_empty_table("memory_relationships", Self::relationships_schema())
                .execute()
                .await?;

            let rel_table = db.open_table("memory_relationships").execute().await?;

            // Scalar indexes for relationships — enable fast lookups by source/target/project
            rel_table
                .create_index(&["source_id"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memory_relationships.source_id")?;
            rel_table
                .create_index(&["target_id"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memory_relationships.target_id")?;
            rel_table
                .create_index(&["project_key"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context("Failed to create Bitmap index on memory_relationships.project_key")?;
            rel_table
                .create_index(&["relationship_type"], Index::Bitmap(Default::default()))
                .execute()
                .await
                .context(
                    "Failed to create Bitmap index on memory_relationships.relationship_type",
                )?;

            tracing::info!("Created Bitmap indexes on memory_relationships table");
        }

        Ok(())
    }

    /// Store a memory
    pub async fn store_memory(&self, memory: &Memory) -> Result<()> {
        // Generate embedding using the optimized single embedding function for better performance
        let searchable_text = memory.get_searchable_text();

        // Validate that we have text to embed
        if searchable_text.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "Cannot generate embedding: searchable text is empty. Title: '{}', Content: '{}'",
                memory.title,
                memory.content
            ));
        }

        let embedding = crate::embedding::generate_embedding(
            &searchable_text,
            self.embedding_provider.as_ref(),
        )
        .await?;

        self.store_memory_with_embedding(memory, embedding).await
    }

    /// Store a memory with a pre-computed embedding (for batch operations)
    async fn store_memory_with_embedding(
        &self,
        memory: &Memory,
        embedding: Vec<f32>,
    ) -> Result<()> {
        // Prepare data
        let tags_json = serde_json::to_string(&memory.metadata.tags)?;
        let files_json = serde_json::to_string(&memory.metadata.related_files)?;

        let embedding_values = Float32Array::from(embedding);
        let embedding_array = FixedSizeListArray::new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            self.vector_dim as i32,
            Arc::new(embedding_values),
            None,
        );

        let batch = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![memory.id.clone()])),
                Arc::new(StringArray::from(vec![self
                    .project_key
                    .as_deref()
                    .unwrap_or("default")
                    .to_string()])),
                Arc::new(StringArray::from(vec![self
                    .role
                    .clone()
                    .unwrap_or_default()])),
                Arc::new(StringArray::from(vec![memory.memory_type.to_string()])),
                Arc::new(StringArray::from(vec![memory.title.clone()])),
                Arc::new(StringArray::from(vec![memory.content.clone()])),
                Arc::new(StringArray::from(vec![memory.created_at.to_rfc3339()])),
                Arc::new(StringArray::from(vec![memory.updated_at.to_rfc3339()])),
                Arc::new(Float32Array::from(vec![memory.metadata.importance])),
                Arc::new(Float32Array::from(vec![memory.metadata.confidence])),
                Arc::new(StringArray::from(vec![tags_json])),
                Arc::new(StringArray::from(vec![files_json])),
                Arc::new(StringArray::from(vec![memory.metadata.git_commit.clone()])),
                Arc::new(StringArray::from(vec![memory.metadata.source.to_string()])),
                Arc::new(Int32Array::from(vec![
                    memory.metadata.decay.access_count as i32,
                ])),
                Arc::new(StringArray::from(vec![memory
                    .metadata
                    .decay
                    .last_accessed
                    .to_rfc3339()])),
                Arc::new(StringArray::from(vec![memory.metadata.state.to_string()])),
                Arc::new(embedding_array),
            ],
        )?;

        // Use merge_insert for atomic upsert (update if exists, insert if not)
        // Key on "id" which is globally unique (UUID)
        use arrow::record_batch::RecordBatchIterator;
        use std::iter::once;
        let batch_reader = RecordBatchIterator::new(once(Ok(batch)), self.schema.clone());
        let mut merge = self.memories_table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(Box::new(batch_reader)).await?;

        Ok(())
    }

    /// Update an existing memory
    pub async fn update_memory(&self, memory: &Memory) -> Result<()> {
        // store_memory upserts via merge_insert keyed on id, so it handles updates too.
        self.store_memory(memory).await
    }

    /// Delete a memory by ID
    pub async fn delete_memory(&self, memory_id: &str) -> Result<()> {
        let id = escape_sql(memory_id);
        let project = escape_sql(self.project_label());

        self.memories_table
            .delete(&format!("id = '{}' AND project_key = '{}'", id, project))
            .await?;

        // Also delete any relationships involving this memory (scoped to project)
        self.relationships_table
            .delete(&format!(
                "(source_id = '{}' OR target_id = '{}') AND project_key = '{}'",
                id, id, project
            ))
            .await
            .ok();

        Ok(())
    }

    /// Periodic ingest-time maintenance. Combines:
    ///
    /// 1. `ensure_optimal_index` — builds the IVF_PQ index once row count
    ///    crosses the threshold (1000 rows by default).
    /// 2. `Table::optimize(OptimizeAction::All)` — folds newly-inserted rows
    ///    into the existing index incrementally (cheap, no retraining) AND
    ///    compacts the many small files LanceDB writes per insert.
    ///
    /// Without this called periodically during a high-write workload (e.g.
    /// ingesting ~25K memories), vector search progressively slows down
    /// because new rows live in an unindexed "delta" that gets linearly
    /// scanned alongside the indexed region. Per LanceDB docs:
    /// <https://lancedb.com/docs/indexing/reindexing/>
    pub async fn run_maintenance(&self) -> Result<()> {
        self.ensure_optimal_index().await?;
        // OptimizeAction::All = Compact + Index incremental + Prune. The
        // Index part is the one that absorbs the unindexed delta into the
        // existing IVF index without retraining. Compact merges small files.
        self.memories_table.optimize(OptimizeAction::All).await?;
        Ok(())
    }

    /// Ensure optimal vector index for memories table (call periodically, not on every store)
    pub async fn ensure_optimal_index(&self) -> Result<()> {
        use crate::vector_optimizer::VectorOptimizer;

        let row_count = self.memories_table.count_rows(None).await?;
        let has_index = self
            .memories_table
            .list_indices()
            .await?
            .iter()
            .any(|idx| idx.columns == vec!["embedding"]);

        if !has_index {
            // Use intelligent optimizer to determine optimal index parameters
            let index_params = VectorOptimizer::calculate_index_params(row_count, self.vector_dim);

            if index_params.should_create_index {
                tracing::info!(
                    "Creating optimized vector index for memories table: {} rows, {} partitions, {} sub-vectors",
                    row_count,
                    index_params.num_partitions,
                    index_params.num_sub_vectors
                );
                self.create_vector_index(index_params).await?;
            } else {
                tracing::debug!(
                    "Skipping index creation for memories table with {} rows - brute force will be faster",
                    row_count
                );
            }
        } else if VectorOptimizer::should_optimize_for_growth(row_count, self.vector_dim, true) {
            // Existing index, dataset grew: rebuild with parameters sized to the new row count.
            tracing::info!("Dataset growth detected, optimizing memories index");
            let index_params = VectorOptimizer::calculate_index_params(row_count, self.vector_dim);
            if index_params.should_create_index {
                self.create_vector_index(index_params).await?;
            }
        }

        Ok(())
    }

    /// Get a memory by ID
    pub async fn get_memory(&self, memory_id: &str) -> Result<Option<Memory>> {
        let id = escape_sql(memory_id);
        let mut results = self
            .memories_table
            .query()
            .only_if(match self.project_key.as_deref() {
                Some(key) => format!("id = '{}' AND project_key = '{}'", id, escape_sql(key)),
                None => format!("id = '{}'", id),
            })
            .limit(1)
            .execute()
            .await?;

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() > 0 {
                let memories = self.batch_to_memories(&batch)?;
                return Ok(memories.into_iter().next());
            }
        }

        Ok(None)
    }

    /// Search memories using vector similarity and optional filters.
    /// Uses hybrid search when enabled (vector + keyword + recency + importance).
    /// If reranker is enabled, it is applied as a final post-processing step on
    /// whichever search path ran (hybrid or vector).
    pub async fn search_memories(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>> {
        // Determine if reranker should run (needs non-empty query text).
        // Read the enabled flag under a short critical section — we drop the
        // guard before any await to keep this safe with the sync Mutex.
        let reranker_enabled = self
            .reranker_integration
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|r| r.config.enabled))
            .unwrap_or(false);
        let reranker_query_text = if reranker_enabled {
            query
                .query_text
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(|t| t.to_string())
        } else {
            None
        };

        // Fetch candidates from the appropriate search path
        let candidates = if self.main_config.search.hybrid.enabled && query.query_text.is_some() {
            // Hybrid path: when reranker is active, fetch more candidates so it has
            // enough material to rerank; otherwise use the normal hybrid limit.
            let mut hybrid_query = self.convert_to_hybrid_query(query);
            if reranker_query_text.is_some() {
                let top_k = self.main_config.search.reranker.top_k_candidates;
                if top_k > 1 {
                    hybrid_query.filters.limit = Some(top_k);
                }
            }
            self.hybrid_search(&hybrid_query).await?
        } else if reranker_query_text.is_some() {
            // Vector-only path with reranker: fetch extended candidate set
            let top_k = self.main_config.search.reranker.top_k_candidates;
            let mut extended_query = query.clone();
            if top_k > 1 {
                extended_query.limit = Some(top_k);
            }
            self.vector_search(&extended_query).await?
        } else {
            // Standard vector search, no reranker
            let results = self.vector_search(query).await?;
            self.record_accesses_best_effort(&results).await;
            return Ok(results);
        };

        // Apply reranker as a post-processing step if enabled. We clone the
        // RerankerIntegration out of the mutex (it's cheap — just config + a
        // small wrapper) so the lock isn't held across the await.
        let reranker_clone = if reranker_query_text.is_some() {
            self.reranker_integration
                .lock()
                .ok()
                .and_then(|g| g.as_ref().cloned())
        } else {
            None
        };
        let final_results =
            if let (Some(query_text), Some(reranker)) = (reranker_query_text, reranker_clone) {
                reranker.rerank_memories(&query_text, candidates).await?
            } else {
                candidates
            };

        self.record_accesses_best_effort(&final_results).await;
        Ok(final_results)
    }

    /// Bump access_count and last_accessed for the memories that this query actually
    /// returned to the caller. Best-effort: failures are logged and swallowed because
    /// failing a search just because the bookkeeping write failed would be worse than
    /// silently missing one access tick.
    ///
    /// Uses LanceDB partial column update so the embedding column is never rewritten —
    /// no re-embedding cost on the read path.
    async fn record_accesses_best_effort(&self, results: &[MemorySearchResult]) {
        if results.is_empty() {
            return;
        }
        let ids: Vec<&str> = results.iter().map(|r| r.memory.id.as_str()).collect();
        if let Err(e) = self.record_accesses(&ids).await {
            tracing::warn!("record_accesses failed (search still succeeded): {}", e);
        }
    }

    /// Apply a lifecycle transition + importance change to one memory without
    /// touching its embedding column. Used by goal-anchored consolidation when
    /// source memories are archived (state → Consolidated, importance dampened).
    pub async fn update_state_and_importance(
        &self,
        id: &str,
        new_state: super::types::MemoryState,
        new_importance: f32,
    ) -> Result<()> {
        let project = escape_sql(self.project_label());
        let id_escaped = escape_sql(id);
        let predicate = format!("id = '{}' AND project_key = '{}'", id_escaped, project);
        let clamped = new_importance.clamp(0.0, 1.0);

        self.memories_table
            .update()
            .only_if(predicate)
            .column("state", format!("'{}'", new_state))
            .column("importance", format!("CAST({} AS FLOAT)", clamped))
            .execute()
            .await
            .context("partial update of state/importance failed")?;
        Ok(())
    }

    /// Bump access_count and last_accessed for the given memory IDs.
    /// Partial update: embedding column is untouched.
    async fn record_accesses(&self, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let id_list = ids
            .iter()
            .map(|id| format!("'{}'", escape_sql(id)))
            .collect::<Vec<_>>()
            .join(",");
        let project = escape_sql(self.project_label());
        let predicate = format!("id IN ({}) AND project_key = '{}'", id_list, project);
        let now_literal = format!("'{}'", Utc::now().to_rfc3339());

        self.memories_table
            .update()
            .only_if(predicate)
            .column("access_count", "access_count + 1")
            .column("last_accessed", now_literal)
            .execute()
            .await
            .context("partial update of access_count/last_accessed failed")?;
        Ok(())
    }

    /// Standard vector search with temporal importance decay.
    /// Scalar filters (memory_type, importance, confidence, git_commit, created_at) are
    /// pushed down to LanceDB via `only_if()`. JSON-serialized fields (tags, related_files)
    /// are filtered in Rust after fetch since they can't be queried natively.
    async fn vector_search(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>> {
        let limit = query
            .limit
            .unwrap_or(self.config.max_search_results)
            .min(self.config.max_search_results);
        let min_relevance = query.min_relevance.unwrap_or(0.0);

        let mut results = Vec::new();

        // Build scalar filter predicate for pushdown (tags/related_files stay in Rust)
        let predicate =
            build_scalar_predicate(self.project_key.as_deref(), self.role.as_deref(), query);

        if let Some(ref query_text) = query.query_text {
            let raw_embedding = self
                .embedding_provider
                .generate_embedding(query_text)
                .await?;
            let query_embedding = self
                .expand_query_embedding(raw_embedding, &predicate)
                .await?;

            let mut db_query = self
                .memories_table
                .vector_search(query_embedding.as_slice())?
                .distance_type(DistanceType::Cosine)
                .limit(limit * 2); // over-fetch to absorb post-filter losses
            if !predicate.is_empty() {
                db_query = db_query.only_if(predicate.clone());
            }

            let mut db_results = db_query.execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let distance_array = f32_column_opt(&batch, "_distance")
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect::<Vec<f32>>())
                    .unwrap_or_default();

                let memories = self.batch_to_memories(&batch)?;

                for (memory, distance) in memories.into_iter().zip(distance_array) {
                    // Only JSON-field filters remain here
                    if !self.matches_json_filters(&memory, query) {
                        continue;
                    }

                    // Cosine distance → similarity, weighted by temporal importance and trust tier
                    let vector_similarity = 1.0 - distance;
                    let current_importance = self.current_importance(&memory);
                    let trust_multiplier = memory.metadata.source.trust_multiplier();
                    let final_score = vector_similarity * current_importance * trust_multiplier;

                    if final_score >= min_relevance {
                        results.push(MemorySearchResult {
                            memory,
                            relevance_score: final_score,
                            selection_reason: self.generate_selection_reason(query, final_score),
                        });
                    }
                }
            }
        } else {
            // No text query — filter-only scan (project_key predicate omitted when unscoped)
            let mut q = self.memories_table.query();
            if !predicate.is_empty() {
                q = q.only_if(predicate);
            }
            let mut db_results = q.execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let memories = self.batch_to_memories(&batch)?;

                for memory in memories {
                    if !self.matches_json_filters(&memory, query) {
                        continue;
                    }

                    let relevance_score = self.current_importance(&memory);

                    if relevance_score >= min_relevance {
                        results.push(MemorySearchResult {
                            memory,
                            relevance_score,
                            selection_reason: self
                                .generate_selection_reason(query, relevance_score),
                        });
                    }
                }
            }
        }

        // Sort
        if let Some(sort_by) = &query.sort_by {
            let sort_order = query
                .sort_order
                .as_ref()
                .unwrap_or(&super::types::SortOrder::Descending);

            results.sort_by(|a, b| {
                let ordering = match sort_by {
                    super::types::MemorySortBy::CreatedAt => {
                        a.memory.created_at.cmp(&b.memory.created_at)
                    }
                    super::types::MemorySortBy::Importance => {
                        let a_imp = self.current_importance(&a.memory);
                        let b_imp = self.current_importance(&b.memory);
                        a_imp
                            .partial_cmp(&b_imp)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }
                };

                match sort_order {
                    super::types::SortOrder::Ascending => ordering,
                    super::types::SortOrder::Descending => ordering.reverse(),
                }
            });
        } else {
            super::types::sort_by_relevance_desc(&mut results);
        }

        results.truncate(limit);
        Ok(results)
    }

    // ===== Recency Scoring =====

    /// Calculate days since memory creation
    fn days_since_creation(memory: &Memory) -> f32 {
        let now = Utc::now();
        let duration = now - memory.created_at;
        duration.num_days() as f32
    }

    /// Exponential recency decay: score = exp(-days_old / decay_days)
    /// Returns [0.0, 1.0] where 1.0 = created today.
    pub(crate) fn calculate_recency_score(memory: &Memory, recency_decay_days: u32) -> f32 {
        let days_old = Self::days_since_creation(memory);
        if days_old < 0.0 {
            return 1.0; // future timestamp — treat as brand new
        }
        let decay_rate = days_old / recency_decay_days as f32;
        (-decay_rate).exp()
    }

    /// Pseudo-relevance feedback (Rocchio) query expansion.
    /// First-pass vector retrieval → centroid of top-K embeddings → blend with original query.
    /// Returns the input embedding unchanged when HyDE is disabled or no neighbors exist.
    async fn expand_query_embedding(
        &self,
        query_embedding: Vec<f32>,
        predicate: &str,
    ) -> Result<Vec<f32>> {
        let hyde = &self.main_config.search.hyde;
        if !hyde.enabled || hyde.top_k == 0 || hyde.alpha >= 1.0 {
            return Ok(query_embedding);
        }

        let mut q = self
            .memories_table
            .vector_search(query_embedding.as_slice())?
            .distance_type(DistanceType::Cosine)
            .limit(hyde.top_k);
        if !predicate.is_empty() {
            q = q.only_if(predicate);
        }
        let mut results = q.execute().await?;

        let dim = query_embedding.len();
        let mut centroid = vec![0.0_f32; dim];
        let mut count = 0usize;

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }
            let Some(emb_col) = batch.column_by_name("embedding") else {
                continue;
            };
            let Some(list_arr) = emb_col.as_any().downcast_ref::<FixedSizeListArray>() else {
                continue;
            };
            for i in 0..list_arr.len() {
                let vec_arr = list_arr.value(i);
                let Some(f32_arr) = vec_arr.as_any().downcast_ref::<Float32Array>() else {
                    continue;
                };
                if f32_arr.len() != dim {
                    continue; // skip mismatched-dim rows defensively
                }
                for (j, c) in centroid.iter_mut().enumerate() {
                    *c += f32_arr.value(j);
                }
                count += 1;
            }
        }

        if count == 0 {
            return Ok(query_embedding);
        }

        let inv = 1.0 / count as f32;
        for c in centroid.iter_mut() {
            *c *= inv;
        }

        Ok(rocchio_blend(&query_embedding, &centroid, hyde.alpha))
    }

    /// Convert MemoryQuery to HybridSearchQuery using config weights
    fn convert_to_hybrid_query(&self, query: &MemoryQuery) -> super::types::HybridSearchQuery {
        let hybrid_config = &self.main_config.search.hybrid;

        super::types::HybridSearchQuery {
            vector_query: query.query_text.clone(),
            vector_weight: hybrid_config.default_vector_weight,
            recency_weight: hybrid_config.default_recency_weight,
            importance_weight: hybrid_config.default_importance_weight,
            filters: query.clone(),
        }
    }

    // ===== Hybrid Search =====

    /// Hybrid search using LanceDB native BM25 + vector RRF fusion.
    ///
    /// LanceDB's `execute_hybrid()` runs vector search and full-text search (BM25/Tantivy)
    /// in parallel and fuses their ranked lists with Reciprocal Rank Fusion (k=60).
    /// The resulting `_relevance_score` is then weighted with recency and importance signals.
    pub async fn hybrid_search(
        &self,
        query: &super::types::HybridSearchQuery,
    ) -> Result<Vec<super::types::MemorySearchResult>> {
        query
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid hybrid query: {}", e))?;

        // query_text is guaranteed Some by validate()
        let query_text = query.vector_query.as_deref().unwrap();

        let limit = query
            .filters
            .limit
            .unwrap_or(self.config.max_search_results);
        let min_relevance = query.filters.min_relevance.unwrap_or(0.0);

        let raw_embedding = self
            .embedding_provider
            .generate_embedding(query_text)
            .await?;

        // Build scalar predicate for pushdown (project_key=None means all projects)
        let predicate = build_scalar_predicate(
            self.project_key.as_deref(),
            self.role.as_deref(),
            &query.filters,
        );

        let query_embedding = self
            .expand_query_embedding(raw_embedding, &predicate)
            .await?;

        let mut db_query = self
            .memories_table
            .vector_search(query_embedding.as_slice())?
            .distance_type(DistanceType::Cosine)
            .limit(limit)
            .full_text_search(FullTextSearchQuery::new(query_text.to_string()));
        if !predicate.is_empty() {
            db_query = db_query.only_if(predicate);
        }

        // RRF fusion: LanceDB combines vector ranks + BM25 ranks internally
        let mut db_results = db_query
            .execute_hybrid(QueryExecutionOptions::default())
            .await?;

        // Max possible RRF score = 2/k (rank 0 in both vector and FTS)
        let max_rrf_score = 2.0 / RRF_K;

        let recency_decay_days = self.main_config.search.hybrid.recency_decay_days;
        let mut results = Vec::new();

        while let Some(batch) = db_results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }

            // Hybrid search returns _relevance_score (raw RRF), not _distance
            let rrf_scores: Vec<f32> = f32_column_opt(&batch, "_relevance_score")
                .map(|arr| {
                    (0..arr.len())
                        .map(|i| (arr.value(i) / max_rrf_score).min(1.0))
                        .collect()
                })
                .unwrap_or_else(|| vec![0.5; batch.num_rows()]);

            let memories = self.batch_to_memories(&batch)?;

            for (memory, rrf_score) in memories.into_iter().zip(rrf_scores) {
                // JSON-field filters (tags, related_files) applied post-fetch
                if !self.matches_json_filters(&memory, &query.filters) {
                    continue;
                }

                let recency_score = Self::calculate_recency_score(&memory, recency_decay_days);
                let importance_score = self.current_importance(&memory);

                // RRF already fuses vector + BM25; recency and importance are additive signals
                // Trust multiplier boosts user-confirmed memories above agent-inferred ones
                let trust_multiplier = memory.metadata.source.trust_multiplier();
                let final_score = (query.vector_weight * rrf_score
                    + query.recency_weight * recency_score
                    + query.importance_weight * importance_score)
                    * trust_multiplier;

                if final_score >= min_relevance {
                    let selection_reason = format!(
                        "Hybrid: rrf={:.2}, recency={:.2}, importance={:.2}, final={:.2}",
                        rrf_score, recency_score, importance_score, final_score
                    );
                    results.push(super::types::MemorySearchResult {
                        memory,
                        relevance_score: final_score,
                        selection_reason,
                    });
                }
            }
        }

        super::types::sort_by_relevance_desc(&mut results);
        results.truncate(limit);

        Ok(results)
    }

    /// Fetch all Working-state memories created on or after `since`. Used by
    /// sleep consolidation to scope the clustering pass to recent activity.
    pub async fn get_recent_working_memories(
        &self,
        since: chrono::DateTime<Utc>,
    ) -> Result<Vec<Memory>> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(key) = self.project_key.as_deref() {
            parts.push(format!("project_key = '{}'", escape_sql(key)));
        }
        if let Some(role) = self.role.as_deref() {
            parts.push(format!("role = '{}'", escape_sql(role)));
        }
        parts.push("state = 'working'".to_string());
        parts.push(format!("created_at >= '{}'", since.to_rfc3339()));
        let filter = parts.join(" AND ");

        let mut q = self.memories_table.query();
        if !filter.is_empty() {
            q = q.only_if(filter);
        }
        let mut results = q.execute().await?;

        let mut memories = Vec::new();
        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }
            memories.extend(self.batch_to_memories(&batch)?);
        }
        Ok(memories)
    }

    /// Store a memory relationship
    pub async fn store_relationship(&self, relationship: &MemoryRelationship) -> Result<()> {
        let batch = RecordBatch::try_new(
            self.rel_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![relationship.id.clone()])),
                Arc::new(StringArray::from(vec![relationship.source_id.clone()])),
                Arc::new(StringArray::from(vec![relationship.target_id.clone()])),
                Arc::new(StringArray::from(vec![self
                    .project_key
                    .as_deref()
                    .unwrap_or("default")
                    .to_string()])),
                Arc::new(StringArray::from(vec![relationship
                    .relationship_type
                    .to_string()])),
                Arc::new(Float32Array::from(vec![relationship.strength])),
                Arc::new(StringArray::from(vec![relationship.description.clone()])),
                Arc::new(StringArray::from(vec![relationship
                    .created_at
                    .to_rfc3339()])),
            ],
        )?;

        // Use merge_insert for atomic upsert (update if exists, insert if not)
        // Key on "id" which is globally unique (UUID)
        use arrow::record_batch::RecordBatchIterator;
        use std::iter::once;
        let batch_reader = RecordBatchIterator::new(once(Ok(batch)), self.rel_schema.clone());
        let mut merge = self.relationships_table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(Box::new(batch_reader)).await?;

        Ok(())
    }

    /// Get relationships for a memory
    pub async fn get_memory_relationships(
        &self,
        memory_id: &str,
    ) -> Result<Vec<MemoryRelationship>> {
        let id = escape_sql(memory_id);
        let mut results = self
            .relationships_table
            .query()
            .only_if(match self.project_key.as_deref() {
                Some(key) => format!(
                    "(source_id = '{}' OR target_id = '{}') AND project_key = '{}'",
                    id,
                    id,
                    escape_sql(key)
                ),
                None => format!("source_id = '{}' OR target_id = '{}'", id, id),
            })
            .execute()
            .await?;

        let mut relationships = Vec::new();

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }

            let mut batch_relationships = self.batch_to_relationships(&batch)?;
            relationships.append(&mut batch_relationships);
        }

        Ok(relationships)
    }

    /// Delete all AutoLinked relationships for a memory (used before re-linking on update)
    pub async fn delete_auto_linked_relationships(&self, memory_id: &str) -> Result<()> {
        let id = escape_sql(memory_id);
        self.relationships_table
            .delete(&format!(
                "(source_id = '{}' OR target_id = '{}') AND relationship_type = 'auto_linked' AND project_key = '{}'",
                id, id, escape_sql(self.project_label())
            ))
            .await
            .ok();
        Ok(())
    }

    /// Get total count of memories (all projects when project_key is None)
    pub async fn get_memory_count(&self) -> Result<usize> {
        let filter = self
            .project_key
            .as_deref()
            .map(|k| format!("project_key = '{}'", escape_sql(k)));
        Ok(self.memories_table.count_rows(filter).await?)
    }

    /// Get distinct project_key and role values across all stored memories
    pub async fn get_distinct_projects_and_roles(&self) -> Result<(Vec<String>, Vec<String>)> {
        let mut q = self.memories_table.query();
        if let Some(key) = self.project_key.as_deref() {
            q = q.only_if(format!("project_key = '{}'", escape_sql(key)));
        }
        let mut results = q.execute().await?;

        let mut projects = std::collections::HashSet::new();
        let mut roles = std::collections::HashSet::new();

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }
            if let Some(col) = string_column_opt(&batch, "project_key") {
                for i in 0..col.len() {
                    if !col.is_null(i) {
                        projects.insert(col.value(i).to_string());
                    }
                }
            }
            if let Some(col) = string_column_opt(&batch, "role") {
                for i in 0..col.len() {
                    if !col.is_null(i) {
                        let v = col.value(i);
                        if !v.is_empty() {
                            roles.insert(v.to_string());
                        }
                    }
                }
            }
        }

        let mut projects: Vec<String> = projects.into_iter().collect();
        let mut roles: Vec<String> = roles.into_iter().collect();
        projects.sort();
        roles.sort();
        Ok((projects, roles))
    }

    /// Get all memories that have non-empty related_files (for stale reference cleanup).
    /// Returns (id, related_files, importance) tuples to avoid loading full embeddings.
    pub async fn get_memories_with_files(&self) -> Result<Vec<Memory>> {
        let filter = match self.project_key.as_deref() {
            Some(key) => format!(
                "project_key = '{}' AND related_files IS NOT NULL AND related_files != '[]'",
                escape_sql(key)
            ),
            None => "related_files IS NOT NULL AND related_files != '[]'".to_string(),
        };

        let mut results = self
            .memories_table
            .query()
            .only_if(filter)
            .execute()
            .await?;

        let mut memories = Vec::new();
        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }
            memories.extend(self.batch_to_memories(&batch)?);
        }

        Ok(memories)
    }

    /// Clean up old memories based on configuration
    pub async fn cleanup_old_memories(&self) -> Result<usize> {
        if let Some(cleanup_days) = self.config.auto_cleanup_days {
            let cutoff_date = Utc::now() - chrono::Duration::days(cleanup_days as i64);
            let cutoff_str = cutoff_date.to_rfc3339();

            let filter = format!(
                "project_key = '{}' AND created_at < '{}' AND importance < {}",
                escape_sql(self.project_label()),
                cutoff_str,
                self.config.cleanup_min_importance
            );

            // Count memories to be deleted
            let mut count_results = self
                .memories_table
                .query()
                .only_if(filter.clone())
                .execute()
                .await?;

            let mut count = 0;
            while let Some(batch) = count_results.try_next().await? {
                count += batch.num_rows();
            }

            // Delete old memories
            self.memories_table.delete(&filter).await?;

            // Optimize table after deletion (compact files, prune deleted rows)
            self.memories_table.optimize(OptimizeAction::All).await?;

            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// Convert RecordBatch to Vec<Memory>
    fn batch_to_memories(&self, batch: &RecordBatch) -> Result<Vec<Memory>> {
        use chrono::DateTime;

        let num_rows = batch.num_rows();
        let mut memories = Vec::with_capacity(num_rows);

        // Extract all columns (required ones error if missing or mistyped)
        let id_array = string_column(batch, "id")?;
        let memory_type_array = string_column(batch, "memory_type")?;
        let title_array = string_column(batch, "title")?;
        let content_array = string_column(batch, "content")?;
        let created_at_array = string_column(batch, "created_at")?;
        let updated_at_array = string_column(batch, "updated_at")?;
        let importance_array = f32_column(batch, "importance")?;
        let confidence_array = f32_column(batch, "confidence")?;
        let tags_array = string_column(batch, "tags")?;
        let files_array = string_column(batch, "related_files")?;
        let git_array = string_column(batch, "git_commit")?;

        // source column may be absent in older databases — fall back to AgentInferred
        let source_array = string_column_opt(batch, "source");

        // Decay columns are present on tables migrated by migrate_decay_columns(); fall
        // back to defaults (count=0, last_accessed=created_at) if absent (e.g. mid-migration).
        let access_count_array = i32_column_opt(batch, "access_count");
        let last_accessed_array = string_column_opt(batch, "last_accessed");
        // State column is added by migrate_state_column on existing tables; default to
        // Working if absent so legacy rows keep their normal retrieval behavior.
        let state_array = string_column_opt(batch, "state");

        for i in 0..num_rows {
            let memory_type =
                super::types::MemoryType::from(memory_type_array.value(i).to_string());

            let tags: Vec<String> = if tags_array.is_null(i) {
                Vec::new()
            } else {
                serde_json::from_str(tags_array.value(i)).unwrap_or_default()
            };

            let related_files: Vec<String> = if files_array.is_null(i) {
                Vec::new()
            } else {
                serde_json::from_str(files_array.value(i)).unwrap_or_default()
            };

            let git_commit = if git_array.is_null(i) {
                None
            } else {
                Some(git_array.value(i).to_string())
            };

            let source = source_array
                .map(|arr| super::types::MemorySource::from(arr.value(i).to_string()))
                .unwrap_or_default();

            let created_at =
                DateTime::parse_from_rfc3339(created_at_array.value(i))?.with_timezone(&Utc);

            let access_count = access_count_array
                .map(|a| a.value(i).max(0) as u32)
                .unwrap_or(0);
            let last_accessed = last_accessed_array
                .and_then(|a| DateTime::parse_from_rfc3339(a.value(i)).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or(created_at);

            let importance = importance_array.value(i);
            let mut decay = super::types::MemoryDecay::new(importance);
            decay.access_count = access_count;
            decay.last_accessed = last_accessed;

            let state = state_array
                .map(|a| super::types::MemoryState::from(a.value(i).to_string()))
                .unwrap_or_default();

            let metadata = super::types::MemoryMetadata {
                git_commit,
                importance,
                confidence: confidence_array.value(i),
                tags,
                related_files,
                source,
                decay,
                state,
                ..Default::default()
            };

            let memory = Memory {
                id: id_array.value(i).to_string(),
                memory_type,
                title: title_array.value(i).to_string(),
                content: content_array.value(i).to_string(),
                created_at,
                updated_at: DateTime::parse_from_rfc3339(updated_at_array.value(i))?
                    .with_timezone(&Utc),
                metadata,
                relevance_score: None,
            };

            memories.push(memory);
        }

        Ok(memories)
    }

    /// Convert RecordBatch to Vec<MemoryRelationship>
    fn batch_to_relationships(&self, batch: &RecordBatch) -> Result<Vec<MemoryRelationship>> {
        use chrono::DateTime;

        let num_rows = batch.num_rows();
        let mut relationships = Vec::with_capacity(num_rows);

        // Extract all columns (all required)
        let id_array = string_column(batch, "id")?;
        let source_array = string_column(batch, "source_id")?;
        let target_array = string_column(batch, "target_id")?;
        let type_array = string_column(batch, "relationship_type")?;
        let strength_array = f32_column(batch, "strength")?;
        let desc_array = string_column(batch, "description")?;
        let created_array = string_column(batch, "created_at")?;

        for i in 0..num_rows {
            // From<&str> understands both the snake_case form emitted by Display
            // (canonical, written by store_relationship) and the legacy CamelCase
            // form so existing rows round-trip correctly.
            let relationship_type = super::types::RelationshipType::from(type_array.value(i));

            let relationship = MemoryRelationship {
                id: id_array.value(i).to_string(),
                source_id: source_array.value(i).to_string(),
                target_id: target_array.value(i).to_string(),
                relationship_type,
                strength: strength_array.value(i),
                description: desc_array.value(i).to_string(),
                created_at: DateTime::parse_from_rfc3339(created_array.value(i))?
                    .with_timezone(&Utc),
            };

            relationships.push(relationship);
        }

        Ok(relationships)
    }

    /// Filter on JSON-serialized fields that cannot be pushed to LanceDB as SQL predicates.
    /// Scalar fields (memory_type, importance, confidence, git_commit, created_at) are
    /// handled by `build_scalar_predicate()` and pushed down via `only_if()`.
    fn matches_json_filters(&self, memory: &Memory, query: &MemoryQuery) -> bool {
        // tags is stored as a JSON array string — must filter in Rust
        if let Some(ref tags) = query.tags {
            if !tags.iter().any(|tag| memory.metadata.tags.contains(tag)) {
                return false;
            }
        }

        // related_files is stored as a JSON array string — must filter in Rust
        if let Some(ref files) = query.related_files {
            if !files
                .iter()
                .any(|file| memory.metadata.related_files.contains(file))
            {
                return false;
            }
        }

        true
    }

    /// Clear all memory data for the current project
    pub async fn clear_all_memory_data(&self) -> Result<usize> {
        // Get current counts before deletion (scoped to project)
        let memory_count = self.get_memory_count().await.unwrap_or(0);

        let project_key = escape_sql(self.project_label());

        // Count relationships for this project
        let relationship_count = self
            .relationships_table
            .count_rows(Some(format!("project_key = '{}'", project_key)))
            .await
            .unwrap_or(0);

        let total_deleted = memory_count + relationship_count;

        // Delete only this project's memories and relationships
        self.memories_table
            .delete(&format!("project_key = '{}'", project_key))
            .await?;

        self.relationships_table
            .delete(&format!("project_key = '{}'", project_key))
            .await?;
        // Optimize tables after deletion
        self.memories_table.optimize(OptimizeAction::All).await?;
        self.relationships_table
            .optimize(OptimizeAction::All)
            .await?;

        Ok(total_deleted)
    }

    /// Generate selection reason for search results
    fn generate_selection_reason(&self, query: &MemoryQuery, relevance_score: f32) -> String {
        let mut reasons = Vec::new();

        if query.query_text.is_some() {
            reasons.push(format!("Semantic similarity: {:.2}", relevance_score));
        }

        if query.memory_types.is_some() {
            reasons.push("Matches memory type filter".to_string());
        }

        if query.tags.is_some() {
            reasons.push("Contains matching tags".to_string());
        }

        if query.related_files.is_some() {
            reasons.push("Related to specified files".to_string());
        }

        if query.git_commit.is_some() {
            reasons.push("Matches Git commit filter".to_string());
        }

        if reasons.is_empty() {
            "Matches search criteria".to_string()
        } else {
            reasons.join(", ")
        }
    }

    /// Enable reranker with optional model override.
    /// Takes `&self` (mutates via interior `Mutex`) so `MemoryStore` can be
    /// shared as `Arc<Self>` for fire-and-forget tasks.
    pub fn enable_reranker(&self, model: Option<String>) {
        let mut guard = match self.reranker_integration.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(), // recover from poisoning — config-only data
        };
        if let Some(ref mut reranker) = *guard {
            if let Some(m) = model {
                reranker.config.model = m;
            }
            reranker.config.enabled = true;
        } else {
            let mut config = self.main_config.search.reranker.clone();
            config.enabled = true;
            if let Some(m) = model {
                config.model = m;
            }
            *guard = Some(RerankerIntegration::new(config));
        }
    }

    /// Disable reranker
    pub fn disable_reranker(&self) {
        let mut guard = match self.reranker_integration.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(ref mut reranker) = *guard {
            reranker.config.enabled = false;
        }
    }
}

/// Test-only re-export of the private `build_scalar_predicate` function.
#[cfg(test)]
pub fn build_scalar_predicate_test(
    project_key: Option<&str>,
    role: Option<&str>,
    query: &crate::memory::types::MemoryQuery,
) -> String {
    build_scalar_predicate(project_key, role, query)
}
