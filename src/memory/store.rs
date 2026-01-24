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

use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;

// Arrow imports
use arrow_array::{Array, FixedSizeListArray, Float32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};

// LanceDB imports
use futures::TryStreamExt;
use lancedb::{
    connect,
    index::Index,
    query::{ExecutableQuery, QueryBase},
    Connection, DistanceType,
};

use super::types::{Memory, MemoryConfig, MemoryQuery, MemoryRelationship, MemorySearchResult};
use crate::embedding::EmbeddingProvider;

/// LanceDB-based storage for memories with vector search capabilities
pub struct MemoryStore {
    db: Connection,
    embedding_provider: Box<dyn EmbeddingProvider>,
    config: MemoryConfig,
    main_config: crate::config::Config,
    vector_dim: usize,
}

impl MemoryStore {
    /// Create a new memory store
    pub async fn new(
        db_path: &str,
        embedding_provider: Box<dyn EmbeddingProvider>,
        config: MemoryConfig,
        main_config: crate::config::Config,
    ) -> Result<Self> {
        // Connect to LanceDB
        let db = connect(db_path).execute().await?;

        // Get vector dimension from the embedding provider by testing with a short text
        let test_embedding = embedding_provider.generate_embedding("test").await?;
        let vector_dim = test_embedding.len();

        let store = Self {
            db,
            embedding_provider,
            config,
            main_config,
            vector_dim,
        };

        // Initialize tables
        store.initialize_tables().await?;

        // Ensure optimal vector index (only during initialization, not on every store)
        store.ensure_optimal_index().await?;

        Ok(store)
    }

    /// Initialize memory and relationship tables
    async fn initialize_tables(&self) -> Result<()> {
        let table_names = self.db.table_names().execute().await?;

        // Create memories table if it doesn't exist
        if !table_names.contains(&"memories".to_string()) {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("memory_type", DataType::Utf8, false),
                Field::new("title", DataType::Utf8, false),
                Field::new("content", DataType::Utf8, false),
                Field::new("created_at", DataType::Utf8, false),
                Field::new("updated_at", DataType::Utf8, false),
                Field::new("importance", DataType::Float32, false),
                Field::new("confidence", DataType::Float32, false),
                Field::new("tags", DataType::Utf8, true), // JSON serialized
                Field::new("related_files", DataType::Utf8, true), // JSON serialized
                Field::new("git_commit", DataType::Utf8, true),
                Field::new(
                    "embedding",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        self.vector_dim as i32,
                    ),
                    true,
                ),
            ]));

            self.db
                .create_empty_table("memories", schema)
                .execute()
                .await?;
        }

        // Create relationships table if it doesn't exist
        if !table_names.contains(&"memory_relationships".to_string()) {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("source_id", DataType::Utf8, false),
                Field::new("target_id", DataType::Utf8, false),
                Field::new("relationship_type", DataType::Utf8, false),
                Field::new("strength", DataType::Float32, false),
                Field::new("description", DataType::Utf8, false),
                Field::new("created_at", DataType::Utf8, false),
            ]));

            self.db
                .create_empty_table("memory_relationships", schema)
                .execute()
                .await?;
        }

        Ok(())
    }

    /// Store a memory
    pub async fn store_memory(&mut self, memory: &Memory) -> Result<()> {
        // Generate embedding using the optimized single embedding function for better performance
        let embedding =
            crate::embedding::generate_embeddings(&memory.get_searchable_text(), &self.main_config)
                .await?;

        self.store_memory_with_embedding(memory, embedding).await
    }

    /// Store a memory with a pre-computed embedding (for batch operations)
    async fn store_memory_with_embedding(
        &mut self,
        memory: &Memory,
        embedding: Vec<f32>,
    ) -> Result<()> {
        // Create record batch
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
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
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    self.vector_dim as i32,
                ),
                true,
            ),
        ]));

        // Prepare data
        let tags_json = serde_json::to_string(&memory.metadata.tags)?;
        let files_json = serde_json::to_string(&memory.metadata.related_files)?;

        // Create embedding array
        let embedding_values = Float32Array::from(embedding);
        let embedding_array = FixedSizeListArray::new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            self.vector_dim as i32,
            Arc::new(embedding_values),
            None,
        );

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![memory.id.clone()])),
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
                Arc::new(embedding_array),
            ],
        )?;

        // Open table and add the batch
        let table = self.db.open_table("memories").execute().await?;

        // Delete existing memory with same ID if it exists
        table.delete(&format!("id = '{}'", memory.id)).await.ok();

        // Add new memory
        use std::iter::once;
        let batches = once(Ok(batch));
        let batch_reader = arrow::record_batch::RecordBatchIterator::new(batches, schema);
        table.add(batch_reader).execute().await?;

        // Index management moved to separate method for performance

        Ok(())
    }

    /// Update an existing memory
    pub async fn update_memory(&mut self, memory: &Memory) -> Result<()> {
        // Just use store_memory as it handles updates by deleting and re-inserting
        self.store_memory(memory).await
    }

    /// Delete a memory by ID
    pub async fn delete_memory(&mut self, memory_id: &str) -> Result<()> {
        let table = self.db.open_table("memories").execute().await?;
        table.delete(&format!("id = '{}'", memory_id)).await?;

        // Also delete any relationships involving this memory
        let rel_table = self.db.open_table("memory_relationships").execute().await?;
        rel_table
            .delete(&format!(
                "source_id = '{}' OR target_id = '{}'",
                memory_id, memory_id
            ))
            .await
            .ok();

        Ok(())
    }

    /// Ensure optimal vector index for memories table (call periodically, not on every store)
    pub async fn ensure_optimal_index(&self) -> Result<()> {
        let table = self.db.open_table("memories").execute().await?;

        // Get current dataset statistics
        let row_count = table.count_rows(None).await?;
        let has_index = table
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

                table
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
                    table
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
        let table = self.db.open_table("memories").execute().await?;

        let mut results = table
            .query()
            .only_if(format!("id = '{}'", memory_id))
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

    /// Search memories using vector similarity and optional filters
    /// Uses hybrid search when enabled (vector + keyword + recency + importance)
    pub async fn search_memories(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>> {
        // Use hybrid search if enabled and we have a text query
        if self.main_config.search.hybrid.enabled && query.query_text.is_some() {
            return self
                .hybrid_search(&self.convert_to_hybrid_query(query))
                .await;
        }

        // Fall back to standard vector search
        self.vector_search(query).await
    }

    /// Standard vector search with temporal decay
    async fn vector_search(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>> {
        let table = self.db.open_table("memories").execute().await?;

        let limit = query
            .limit
            .unwrap_or(self.config.max_search_results)
            .min(self.config.max_search_results);
        let min_relevance = query.min_relevance.unwrap_or(0.0);

        let mut results = Vec::new();

        // If we have a text query, use semantic search
        if let Some(ref query_text) = query.query_text {
            let query_embedding = self
                .embedding_provider
                .generate_embedding(query_text)
                .await?;

            // Start with optimized vector search
            let mut db_query = table
                .vector_search(query_embedding.as_slice())?
                .distance_type(DistanceType::Cosine)
                .limit(limit * 2); // Get more results to filter

            // Apply intelligent search optimization
            db_query = crate::vector_optimizer::VectorOptimizer::optimize_query(
                db_query, &table, "memories",
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to optimize query: {}", e))?;

            let mut db_results = db_query.execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                // Extract distance column
                let distance_array = batch
                    .column_by_name("_distance")
                    .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
                    .map(|arr| (0..arr.len()).map(|i| arr.value(i)).collect::<Vec<f32>>())
                    .unwrap_or_default();

                let memories = self.batch_to_memories(&batch)?;

                for (memory, distance) in memories.into_iter().zip(distance_array.into_iter()) {
                    // Apply filters
                    if !self.matches_filters(&memory, query) {
                        continue;
                    }

                    // Convert distance to similarity (cosine distance is 1 - similarity)
                    let vector_similarity = 1.0 - distance;

                    // Calculate current importance with decay
                    let current_importance = memory.get_current_importance(
                        self.config.decay_enabled,
                        self.config.min_importance_threshold,
                    );

                    // Combine vector similarity with temporal importance
                    // Final score = vector_similarity * current_importance
                    let final_score = vector_similarity * current_importance;

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
            // No text query, just apply filters
            let mut db_results = table.query().execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let memories = self.batch_to_memories(&batch)?;

                for memory in memories {
                    if self.matches_filters(&memory, query) {
                        // Use current importance with decay
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
        }

        // Apply sorting based on query parameters
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
                        // Use current importance with decay for sorting
                        let a_importance = a.memory.get_current_importance(
                            self.config.decay_enabled,
                            self.config.min_importance_threshold,
                        );
                        let b_importance = b.memory.get_current_importance(
                            self.config.decay_enabled,
                            self.config.min_importance_threshold,
                        );
                        a_importance
                            .partial_cmp(&b_importance)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    }
                };

                match sort_order {
                    super::types::SortOrder::Ascending => ordering,
                    super::types::SortOrder::Descending => ordering.reverse(),
                }
            });
        } else {
            // Default: Sort by relevance score (highest first)
            results.sort_by(|a, b| {
                b.relevance_score
                    .partial_cmp(&a.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Apply final limit
        results.truncate(limit);

        Ok(results)
    }

    // ===== Keyword Search Methods =====

    /// Tokenize text into lowercase words, removing punctuation
    pub(crate) fn tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// Calculate term frequency for a keyword in text
    pub(crate) fn calculate_tf(keyword: &str, text: &str) -> f32 {
        let tokens = Self::tokenize(text);
        if tokens.is_empty() {
            return 0.0;
        }

        let keyword_lower = keyword.to_lowercase();
        let count = tokens.iter().filter(|t| *t == &keyword_lower).count();
        count as f32 / tokens.len() as f32
    }

    /// Score a field (title/content/tags) for keyword matches
    pub(crate) fn score_field(keywords: &[String], text: &str, field_weight: f32) -> f32 {
        if keywords.is_empty() || text.is_empty() {
            return 0.0;
        }

        let mut total_score = 0.0;
        for keyword in keywords {
            let tf = Self::calculate_tf(keyword, text);
            total_score += tf * field_weight;
        }

        total_score
    }

    /// Perform keyword-based search on memories
    /// Returns memories with keyword match scores
    pub async fn keyword_search(
        &self,
        keywords: &[String],
        filters: &super::types::MemoryQuery,
    ) -> Result<Vec<(Memory, f32)>> {
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

        let table = self.db.open_table("memories").execute().await?;
        let mut results = Vec::new();

        // Get all memories (we'll score them)
        let mut db_results = table.query().execute().await?;

        while let Some(batch) = db_results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }

            let memories = self.batch_to_memories(&batch)?;

            for memory in memories {
                // Apply filters
                if !self.matches_filters(&memory, filters) {
                    continue;
                }

                // Calculate keyword score for each field
                let title_score = Self::score_field(keywords, &memory.title, 3.0);
                let content_score = Self::score_field(keywords, &memory.content, 1.0);
                let tags_score = Self::score_field(keywords, &memory.metadata.tags.join(" "), 2.0);

                let total_score = title_score + content_score + tags_score;

                // Only include if there's a match
                if total_score > 0.0 {
                    results.push((memory, total_score));
                }
            }
        }

        // Normalize scores to [0.0, 1.0]
        if !results.is_empty() {
            let max_score = results
                .iter()
                .map(|(_, score)| *score)
                .fold(0.0f32, f32::max);

            if max_score > 0.0 {
                for (_, score) in &mut results {
                    *score /= max_score;
                }
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(results)
    }

    // ===== Recency Scoring Methods =====

    /// Calculate days since memory creation
    fn days_since_creation(memory: &Memory) -> f32 {
        let now = Utc::now();
        let duration = now - memory.created_at;
        duration.num_days() as f32
    }

    /// Calculate recency score using exponential decay
    /// Score = exp(-days_old / decay_days)
    /// Returns value in [0.0, 1.0] where 1.0 = created today
    pub(crate) fn calculate_recency_score(memory: &Memory, recency_decay_days: u32) -> f32 {
        let days_old = Self::days_since_creation(memory);

        // Handle future timestamps (shouldn't happen, but be safe)
        if days_old < 0.0 {
            return 1.0;
        }

        // Exponential decay: e^(-days / decay_period)
        let decay_rate = days_old / recency_decay_days as f32;
        (-decay_rate).exp()
    }

    /// Convert MemoryQuery to HybridSearchQuery using config weights
    fn convert_to_hybrid_query(&self, query: &MemoryQuery) -> super::types::HybridSearchQuery {
        let hybrid_config = &self.main_config.search.hybrid;

        super::types::HybridSearchQuery {
            vector_query: query.query_text.clone(),
            keywords: None, // Could be extracted from query_text in future
            vector_weight: hybrid_config.default_vector_weight,
            keyword_weight: hybrid_config.default_keyword_weight,
            recency_weight: hybrid_config.default_recency_weight,
            importance_weight: hybrid_config.default_importance_weight,
            filters: query.clone(),
        }
    }

    // ===== Hybrid Search Methods =====

    /// Perform hybrid search combining multiple signals
    pub async fn hybrid_search(
        &self,
        query: &super::types::HybridSearchQuery,
    ) -> Result<Vec<super::types::MemorySearchResult>> {
        // Validate query
        query
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid hybrid query: {}", e))?;

        let limit = query
            .filters
            .limit
            .unwrap_or(self.config.max_search_results);
        let min_relevance = query.filters.min_relevance.unwrap_or(0.0);

        // Step 1: Get candidate memories from vector search or keyword search
        let mut candidates: std::collections::HashMap<String, (Memory, f32, f32, f32, f32)> =
            std::collections::HashMap::new();

        // Perform vector search if query provided
        if let Some(ref vector_query) = query.vector_query {
            let vector_results = self
                .vector_search(&super::types::MemoryQuery {
                    query_text: Some(vector_query.clone()),
                    limit: Some(limit * 2), // Get more candidates for filtering
                    ..query.filters.clone()
                })
                .await?;

            for result in vector_results {
                let memory_id = result.memory.id.clone();
                candidates.insert(
                    memory_id,
                    (result.memory, result.relevance_score, 0.0, 0.0, 0.0),
                );
            }
        }

        // Perform keyword search if keywords provided
        if let Some(ref keywords) = query.keywords {
            let keyword_results = self.keyword_search(keywords, &query.filters).await?;

            for (memory, kw_score) in keyword_results {
                let memory_id = memory.id.clone();
                candidates
                    .entry(memory_id)
                    .and_modify(|(_, _vec_score, kw, _, _)| *kw = kw_score)
                    .or_insert((memory, 0.0, kw_score, 0.0, 0.0));
            }
        }

        // If no candidates yet, get all memories (for recency/importance only queries)
        if candidates.is_empty() {
            let table = self.db.open_table("memories").execute().await?;
            let mut db_results = table.query().execute().await?;

            while let Some(batch) = db_results.try_next().await? {
                if batch.num_rows() == 0 {
                    continue;
                }

                let memories = self.batch_to_memories(&batch)?;
                for memory in memories {
                    if self.matches_filters(&memory, &query.filters) {
                        let memory_id = memory.id.clone();
                        candidates.insert(memory_id, (memory, 0.0, 0.0, 0.0, 0.0));
                    }
                }
            }
        }

        // Step 2: Calculate recency and importance scores for all candidates
        let recency_decay_days = self.main_config.search.hybrid.recency_decay_days;
        for (_memory_id, (memory, _vec_score, _kw_score, rec_score, imp_score)) in
            candidates.iter_mut()
        {
            *rec_score = Self::calculate_recency_score(memory, recency_decay_days);
            *imp_score = memory.get_current_importance(
                self.config.decay_enabled,
                self.config.min_importance_threshold,
            );
        }

        // Step 3: Combine scores with weights
        let mut results: Vec<super::types::MemorySearchResult> = candidates
            .into_iter()
            .map(|(_, (memory, vec_score, kw_score, rec_score, imp_score))| {
                // Calculate weighted final score
                let final_score = query.vector_weight * vec_score
                    + query.keyword_weight * kw_score
                    + query.recency_weight * rec_score
                    + query.importance_weight * imp_score;

                // Generate selection reason with signal breakdown
                let selection_reason = format!(
                    "Hybrid: vector={:.2}, keyword={:.2}, recency={:.2}, importance={:.2}, final={:.2}",
                    vec_score, kw_score, rec_score, imp_score, final_score
                );

                super::types::MemorySearchResult {
                    memory,
                    relevance_score: final_score,
                    selection_reason,
                }
            })
            .filter(|result| result.relevance_score >= min_relevance)
            .collect();

        // Step 4: Sort by final score descending
        results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Step 5: Apply limit
        results.truncate(limit);

        Ok(results)
    }
    /// Store a memory relationship
    pub async fn store_relationship(&mut self, relationship: &MemoryRelationship) -> Result<()> {
        let table = self.db.open_table("memory_relationships").execute().await?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, false),
            Field::new("target_id", DataType::Utf8, false),
            Field::new("relationship_type", DataType::Utf8, false),
            Field::new("strength", DataType::Float32, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![relationship.id.clone()])),
                Arc::new(StringArray::from(vec![relationship.source_id.clone()])),
                Arc::new(StringArray::from(vec![relationship.target_id.clone()])),
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

        // Delete existing relationship with same ID if it exists
        table
            .delete(&format!("id = '{}'", relationship.id))
            .await
            .ok();

        // Add new relationship
        use std::iter::once;
        let batches = once(Ok(batch));
        let batch_reader = arrow::record_batch::RecordBatchIterator::new(batches, schema);
        table.add(batch_reader).execute().await?;

        Ok(())
    }

    /// Get relationships for a memory
    pub async fn get_memory_relationships(
        &self,
        memory_id: &str,
    ) -> Result<Vec<MemoryRelationship>> {
        let table = self.db.open_table("memory_relationships").execute().await?;

        let mut results = table
            .query()
            .only_if(format!(
                "source_id = '{}' OR target_id = '{}'",
                memory_id, memory_id
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

    /// Get total count of memories
    pub async fn get_memory_count(&self) -> Result<usize> {
        let table = self.db.open_table("memories").execute().await?;
        Ok(table.count_rows(None).await?)
    }

    /// Clean up old memories based on configuration
    pub async fn cleanup_old_memories(&mut self) -> Result<usize> {
        if let Some(cleanup_days) = self.config.auto_cleanup_days {
            let cutoff_date = Utc::now() - chrono::Duration::days(cleanup_days as i64);
            let cutoff_str = cutoff_date.to_rfc3339();

            let table = self.db.open_table("memories").execute().await?;

            // Count memories to be deleted
            let mut count_results = table
                .query()
                .only_if(format!(
                    "created_at < '{}' AND importance < {}",
                    cutoff_str, self.config.cleanup_min_importance
                ))
                .execute()
                .await?;

            let mut count = 0;
            while let Some(batch) = count_results.try_next().await? {
                count += batch.num_rows();
            }

            // Delete old memories
            table
                .delete(&format!(
                    "created_at < '{}' AND importance < {}",
                    cutoff_str, self.config.cleanup_min_importance
                ))
                .await?;

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

            let metadata = super::types::MemoryMetadata {
                git_commit,
                importance: importance_array.value(i),
                confidence: confidence_array.value(i),
                tags,
                related_files,
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

    /// Check if memory matches the query filters
    fn matches_filters(&self, memory: &Memory, query: &MemoryQuery) -> bool {
        // Filter by memory types
        if let Some(ref memory_types) = query.memory_types {
            if !memory_types.contains(&memory.memory_type) {
                return false;
            }
        }

        // Filter by tags (any of these tags)
        if let Some(ref tags) = query.tags {
            if !tags.iter().any(|tag| memory.metadata.tags.contains(tag)) {
                return false;
            }
        }

        // Filter by related files
        if let Some(ref files) = query.related_files {
            if !files
                .iter()
                .any(|file| memory.metadata.related_files.contains(file))
            {
                return false;
            }
        }

        // Filter by git commit
        if let Some(ref git_commit) = query.git_commit {
            if memory.metadata.git_commit.as_ref() != Some(git_commit) {
                return false;
            }
        }

        // Filter by minimum importance
        if let Some(min_importance) = query.min_importance {
            if memory.metadata.importance < min_importance {
                return false;
            }
        }

        // Filter by minimum confidence
        if let Some(min_confidence) = query.min_confidence {
            if memory.metadata.confidence < min_confidence {
                return false;
            }
        }

        // Filter by creation date range
        if let Some(created_after) = query.created_after {
            if memory.created_at < created_after {
                return false;
            }
        }

        if let Some(created_before) = query.created_before {
            if memory.created_at > created_before {
                return false;
            }
        }

        true
    }

    /// Clear all memory data (memories and relationships)
    pub async fn clear_all_memory_data(&mut self) -> Result<usize> {
        // Get current counts before deletion
        let memory_count = self.get_memory_count().await.unwrap_or(0);

        // Count relationships
        let rel_table = self.db.open_table("memory_relationships").execute().await?;
        let relationship_count = rel_table.count_rows(None).await.unwrap_or(0);

        let total_deleted = memory_count + relationship_count;

        // Drop and recreate memories table
        if self
            .db
            .table_names()
            .execute()
            .await?
            .contains(&"memories".to_string())
        {
            self.db.drop_table("memories", &[]).await?;
        }

        // Drop and recreate relationships table
        if self
            .db
            .table_names()
            .execute()
            .await?
            .contains(&"memory_relationships".to_string())
        {
            self.db.drop_table("memory_relationships", &[]).await?;
        }

        // Recreate tables
        self.initialize_tables().await?;

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
}
