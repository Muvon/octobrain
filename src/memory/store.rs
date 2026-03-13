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
use arrow_array::{Array, FixedSizeListArray, Float32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};

// LanceDB imports
use futures::TryStreamExt;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::{
    connect,
    index::Index,
    query::{ExecutableQuery, QueryBase, QueryExecutionOptions},
    table::OptimizeAction,
    Connection, DistanceType, Table,
};

/// RRF (Reciprocal Rank Fusion) constant k — same value used by knowledge store.
/// Based on: https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf
/// "Experiments indicate that k = 60 was near-optimal"
const RRF_K: f32 = 60.0;

use super::reranker_integration::RerankerIntegration;
use super::types::{Memory, MemoryConfig, MemoryQuery, MemoryRelationship, MemorySearchResult};
use crate::embedding::EmbeddingProvider;

/// Build a SQL predicate string for scalar fields that LanceDB can filter at the storage layer.
///
/// Tags and related_files are excluded here because they are stored as JSON-serialized strings
/// and cannot be queried with simple SQL equality — those are handled post-fetch in Rust.
fn build_scalar_predicate(project_key: &str, role: Option<&str>, query: &MemoryQuery) -> String {
    // project_key is always the first condition to scope all queries to the current project
    let mut parts: Vec<String> = vec![format!("project_key = '{}'", project_key)];

    // role filter — only applied when a role is set (None = no filter)
    if let Some(role) = role {
        parts.push(format!("role = '{}'", role));
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
        // Escape single quotes in the commit hash (defensive, hashes won't have them)
        let escaped = git_commit.replace('\'', "''");
        parts.push(format!("git_commit = '{}'", escaped));
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
    reranker_integration: Option<RerankerIntegration>,
    project_key: String,
    role: Option<String>,
}

impl MemoryStore {
    /// Create a new memory store
    pub async fn new(
        db_path: &str,
        project_key: String,
        role: Option<String>,
        embedding_provider: Box<dyn EmbeddingProvider>,
        config: MemoryConfig,
        main_config: crate::config::Config,
        reranker_integration: Option<RerankerIntegration>,
    ) -> Result<Self> {
        let db = connect(db_path).execute().await?;

        // Get vector dimension from the embedding provider by testing with a short text
        let test_embedding = embedding_provider.generate_embedding("test").await?;
        let vector_dim = test_embedding.len();

        // Build the memories schema once — reused for every write
        let schema = Arc::new(Schema::new(vec![
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
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vector_dim as i32,
                ),
                true,
            ),
        ]));

        // Initialize tables (creates them if missing, adds scalar + FTS indexes)
        Self::init_tables(&db, &schema).await?;

        // Cache table handles — opened once, reused for the lifetime of this store
        let memories_table = db.open_table("memories").execute().await?;
        let relationships_table = db.open_table("memory_relationships").execute().await?;

        // Build relationship schema once — reused for every relationship write
        let rel_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, false),
            Field::new("target_id", DataType::Utf8, false),
            Field::new("project_key", DataType::Utf8, false),
            Field::new("relationship_type", DataType::Utf8, false),
            Field::new("strength", DataType::Float32, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
        ]));

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
            let rel_schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("source_id", DataType::Utf8, false),
                Field::new("target_id", DataType::Utf8, false),
                Field::new("project_key", DataType::Utf8, false),
                Field::new("relationship_type", DataType::Utf8, false),
                Field::new("strength", DataType::Float32, false),
                Field::new("description", DataType::Utf8, false),
                Field::new("created_at", DataType::Utf8, false),
            ]));

            db.create_empty_table("memory_relationships", rel_schema)
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
    pub async fn store_memory(&mut self, memory: &Memory) -> Result<()> {
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
        &mut self,
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
                Arc::new(StringArray::from(vec![self.project_key.clone()])),
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
    pub async fn update_memory(&mut self, memory: &Memory) -> Result<()> {
        // Just use store_memory as it handles updates by deleting and re-inserting
        self.store_memory(memory).await
    }

    /// Delete a memory by ID
    pub async fn delete_memory(&mut self, memory_id: &str) -> Result<()> {
        self.memories_table
            .delete(&format!(
                "id = '{}' AND project_key = '{}'",
                memory_id, self.project_key
            ))
            .await?;

        // Also delete any relationships involving this memory (scoped to project)
        self.relationships_table
            .delete(&format!(
                "(source_id = '{}' OR target_id = '{}') AND project_key = '{}'",
                memory_id, memory_id, self.project_key
            ))
            .await
            .ok();

        Ok(())
    }

    /// Ensure optimal vector index for memories table (call periodically, not on every store)
    pub async fn ensure_optimal_index(&self) -> Result<()> {
        let row_count = self.memories_table.count_rows(None).await?;
        let has_index = self
            .memories_table
            .list_indices()
            .await?
            .iter()
            .any(|idx| idx.columns == vec!["embedding"]);

        if !has_index {
            // Use intelligent optimizer to determine optimal index parameters
            let index_params = crate::vector_optimizer::VectorOptimizer::calculate_index_params(
                row_count,
                self.vector_dim,
            );

            if index_params.should_create_index {
                tracing::info!(
					"Creating optimized vector index for memories table: {} rows, {} partitions, {} sub-vectors",
					row_count, index_params.num_partitions, index_params.num_sub_vectors
				);

                self.memories_table
                    .create_index(
                        &["embedding"],
                        Index::IvfPq(
                            lancedb::index::vector::IvfPqIndexBuilder::default()
                                .distance_type(index_params.distance_type)
                                .num_partitions(index_params.num_partitions)
                                .num_sub_vectors(index_params.num_sub_vectors)
                                .num_bits(index_params.num_bits as u32),
                        ),
                    )
                    .execute()
                    .await?;
            } else {
                tracing::debug!(
					"Skipping index creation for memories table with {} rows - brute force will be faster",
					row_count
				);
            }
        } else {
            // Check if we should optimize existing index due to growth
            if crate::vector_optimizer::VectorOptimizer::should_optimize_for_growth(
                row_count,
                self.vector_dim,
                true,
            ) {
                tracing::info!("Dataset growth detected, optimizing memories index");

                // Recreate index with optimal parameters
                let index_params = crate::vector_optimizer::VectorOptimizer::calculate_index_params(
                    row_count,
                    self.vector_dim,
                );

                if index_params.should_create_index {
                    self.memories_table
                        .create_index(
                            &["embedding"],
                            Index::IvfPq(
                                lancedb::index::vector::IvfPqIndexBuilder::default()
                                    .distance_type(index_params.distance_type)
                                    .num_partitions(index_params.num_partitions)
                                    .num_sub_vectors(index_params.num_sub_vectors)
                                    .num_bits(index_params.num_bits as u32),
                            ),
                        )
                        .execute()
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// Get a memory by ID
    pub async fn get_memory(&self, memory_id: &str) -> Result<Option<Memory>> {
        let mut results = self
            .memories_table
            .query()
            .only_if(format!(
                "id = '{}' AND project_key = '{}'",
                memory_id, self.project_key
            ))
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
        // Determine if reranker should run (needs non-empty query text)
        let reranker_query_text = self
            .reranker_integration
            .as_ref()
            .filter(|r| r.config.enabled)
            .and(query.query_text.as_deref())
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.to_string());

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
            return self.vector_search(query).await;
        };

        // Apply reranker as a post-processing step if enabled
        if let (Some(ref query_text), Some(ref reranker)) =
            (reranker_query_text, &self.reranker_integration)
        {
            return reranker.rerank_memories(query_text, candidates).await;
        }

        Ok(candidates)
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
        let predicate = build_scalar_predicate(&self.project_key, self.role.as_deref(), query);

        if let Some(ref query_text) = query.query_text {
            let query_embedding = self
                .embedding_provider
                .generate_embedding(query_text)
                .await?;

            let db_query = self
                .memories_table
                .vector_search(query_embedding.as_slice())?
                .distance_type(DistanceType::Cosine)
                .limit(limit * 2) // over-fetch to absorb post-filter losses
                .only_if(predicate.clone());

            let mut db_results = db_query.execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let distance_array = batch
                    .column_by_name("_distance")
                    .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect::<Vec<f32>>())
                    .unwrap_or_default();

                let memories = self.batch_to_memories(&batch)?;

                for (memory, distance) in memories.into_iter().zip(distance_array.into_iter()) {
                    // Only JSON-field filters remain here
                    if !self.matches_json_filters(&memory, query) {
                        continue;
                    }

                    // Cosine distance → similarity, weighted by temporal importance and trust tier
                    let vector_similarity = 1.0 - distance;
                    let current_importance = memory.get_current_importance(
                        self.config.decay_enabled,
                        self.config.min_importance_threshold,
                    );
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
            // No text query — filter-only scan (predicate always includes project_key)
            let mut db_results = self
                .memories_table
                .query()
                .only_if(predicate)
                .execute()
                .await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let memories = self.batch_to_memories(&batch)?;

                for memory in memories {
                    if !self.matches_json_filters(&memory, query) {
                        continue;
                    }

                    let relevance_score = memory.get_current_importance(
                        self.config.decay_enabled,
                        self.config.min_importance_threshold,
                    );

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
                        let a_imp = a.memory.get_current_importance(
                            self.config.decay_enabled,
                            self.config.min_importance_threshold,
                        );
                        let b_imp = b.memory.get_current_importance(
                            self.config.decay_enabled,
                            self.config.min_importance_threshold,
                        );
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
            results.sort_by(|a, b| {
                b.relevance_score
                    .partial_cmp(&a.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
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

        let query_embedding = self
            .embedding_provider
            .generate_embedding(query_text)
            .await?;

        // Build scalar predicate for pushdown (always includes project_key)
        let predicate =
            build_scalar_predicate(&self.project_key, self.role.as_deref(), &query.filters);

        let db_query = self
            .memories_table
            .vector_search(query_embedding.as_slice())?
            .distance_type(DistanceType::Cosine)
            .limit(limit)
            .full_text_search(FullTextSearchQuery::new(query_text.to_string()))
            .only_if(predicate);

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
            let rrf_scores: Vec<f32> = batch
                .column_by_name("_relevance_score")
                .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
                .map(|arr| {
                    (0..arr.len())
                        .map(|i| (arr.value(i) / max_rrf_score).min(1.0))
                        .collect()
                })
                .unwrap_or_else(|| vec![0.5; batch.num_rows()]);

            let memories = self.batch_to_memories(&batch)?;

            for (memory, rrf_score) in memories.into_iter().zip(rrf_scores.into_iter()) {
                // JSON-field filters (tags, related_files) applied post-fetch
                if !self.matches_json_filters(&memory, &query.filters) {
                    continue;
                }

                let recency_score = Self::calculate_recency_score(&memory, recency_decay_days);
                let importance_score = memory.get_current_importance(
                    self.config.decay_enabled,
                    self.config.min_importance_threshold,
                );

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

        results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    /// Store a memory relationship
    pub async fn store_relationship(&mut self, relationship: &MemoryRelationship) -> Result<()> {
        let batch = RecordBatch::try_new(
            self.rel_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![relationship.id.clone()])),
                Arc::new(StringArray::from(vec![relationship.source_id.clone()])),
                Arc::new(StringArray::from(vec![relationship.target_id.clone()])),
                Arc::new(StringArray::from(vec![self.project_key.clone()])),
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
        let mut results = self
            .relationships_table
            .query()
            .only_if(format!(
                "(source_id = '{}' OR target_id = '{}') AND project_key = '{}'",
                memory_id, memory_id, self.project_key
            ))
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
    pub async fn delete_auto_linked_relationships(&mut self, memory_id: &str) -> Result<()> {
        self.relationships_table
            .delete(&format!(
                "(source_id = '{}' OR target_id = '{}') AND relationship_type = 'auto_linked' AND project_key = '{}'",
                memory_id, memory_id, self.project_key
            ))
            .await
            .ok();
        Ok(())
    }

    /// Get total count of memories for the current project
    pub async fn get_memory_count(&self) -> Result<usize> {
        Ok(self
            .memories_table
            .count_rows(Some(format!("project_key = '{}'", self.project_key)))
            .await?)
    }

    /// Clean up old memories based on configuration
    pub async fn cleanup_old_memories(&mut self) -> Result<usize> {
        if let Some(cleanup_days) = self.config.auto_cleanup_days {
            let cutoff_date = Utc::now() - chrono::Duration::days(cleanup_days as i64);
            let cutoff_str = cutoff_date.to_rfc3339();

            let filter = format!(
                "project_key = '{}' AND created_at < '{}' AND importance < {}",
                self.project_key, cutoff_str, self.config.cleanup_min_importance
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

        // Extract all columns
        let id_array = batch
            .column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("id column not found or wrong type"))?;

        let memory_type_array = batch
            .column_by_name("memory_type")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("memory_type column not found or wrong type"))?;

        let title_array = batch
            .column_by_name("title")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("title column not found or wrong type"))?;

        let content_array = batch
            .column_by_name("content")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("content column not found or wrong type"))?;

        let created_at_array = batch
            .column_by_name("created_at")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("created_at column not found or wrong type"))?;

        let updated_at_array = batch
            .column_by_name("updated_at")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("updated_at column not found or wrong type"))?;

        let importance_array = batch
            .column_by_name("importance")
            .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow::anyhow!("importance column not found or wrong type"))?;

        let confidence_array = batch
            .column_by_name("confidence")
            .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow::anyhow!("confidence column not found or wrong type"))?;

        let tags_array = batch
            .column_by_name("tags")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("tags column not found or wrong type"))?;

        let files_array = batch
            .column_by_name("related_files")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("related_files column not found or wrong type"))?;

        let git_array = batch
            .column_by_name("git_commit")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("git_commit column not found or wrong type"))?;

        // source column may be absent in older databases — fall back to AgentInferred
        let source_array = batch
            .column_by_name("source")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>());
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

            let metadata = super::types::MemoryMetadata {
                git_commit,
                importance: importance_array.value(i),
                confidence: confidence_array.value(i),
                tags,
                related_files,
                source,
                ..Default::default()
            };

            let memory = Memory {
                id: id_array.value(i).to_string(),
                memory_type,
                title: title_array.value(i).to_string(),
                content: content_array.value(i).to_string(),
                created_at: DateTime::parse_from_rfc3339(created_at_array.value(i))?
                    .with_timezone(&Utc),
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

        // Extract all columns
        let id_array = batch
            .column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("id column not found or wrong type"))?;

        let source_array = batch
            .column_by_name("source_id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("source_id column not found or wrong type"))?;

        let target_array = batch
            .column_by_name("target_id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("target_id column not found or wrong type"))?;

        let type_array = batch
            .column_by_name("relationship_type")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("relationship_type column not found or wrong type"))?;

        let strength_array = batch
            .column_by_name("strength")
            .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| anyhow::anyhow!("strength column not found or wrong type"))?;

        let desc_array = batch
            .column_by_name("description")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("description column not found or wrong type"))?;

        let created_array = batch
            .column_by_name("created_at")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow::anyhow!("created_at column not found or wrong type"))?;

        for i in 0..num_rows {
            let relationship_type = match type_array.value(i) {
                "RelatedTo" => super::types::RelationshipType::RelatedTo,
                "DependsOn" => super::types::RelationshipType::DependsOn,
                "Supersedes" => super::types::RelationshipType::Supersedes,
                "Similar" => super::types::RelationshipType::Similar,
                "Conflicts" => super::types::RelationshipType::Conflicts,
                "Implements" => super::types::RelationshipType::Implements,
                "Extends" => super::types::RelationshipType::Extends,
                other => super::types::RelationshipType::Custom(other.to_string()),
            };

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
    pub async fn clear_all_memory_data(&mut self) -> Result<usize> {
        // Get current counts before deletion (scoped to project)
        let memory_count = self.get_memory_count().await.unwrap_or(0);

        // Count relationships for this project
        let relationship_count = self
            .relationships_table
            .count_rows(Some(format!("project_key = '{}'", self.project_key)))
            .await
            .unwrap_or(0);

        let total_deleted = memory_count + relationship_count;

        // Delete only this project's memories and relationships
        self.memories_table
            .delete(&format!("project_key = '{}'", self.project_key))
            .await?;

        self.relationships_table
            .delete(&format!("project_key = '{}'", self.project_key))
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

    /// Enable reranker with optional model override
    pub fn enable_reranker(&mut self, model: Option<String>) {
        if let Some(ref mut reranker) = self.reranker_integration {
            // Update existing reranker config
            if let Some(m) = model {
                reranker.config.model = m;
            }
            reranker.config.enabled = true;
        } else {
            // Create new reranker integration
            let mut config = self.main_config.search.reranker.clone();
            config.enabled = true;
            if let Some(m) = model {
                config.model = m;
            }
            self.reranker_integration = Some(RerankerIntegration::new(config));
        }
    }

    /// Disable reranker
    pub fn disable_reranker(&mut self) {
        if let Some(ref mut reranker) = self.reranker_integration {
            reranker.config.enabled = false;
        }
    }
}

/// Test-only re-export of the private `build_scalar_predicate` function.
#[cfg(test)]
pub fn build_scalar_predicate_test(
    project_key: &str,
    role: Option<&str>,
    query: &crate::memory::types::MemoryQuery,
) -> String {
    build_scalar_predicate(project_key, role, query)
}
