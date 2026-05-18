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
use chrono::{Duration, Utc};
use std::collections::HashSet;
use std::path::PathBuf;

use super::git_utils::{FileFate, GitUtils, RenameMap};
use super::store::MemoryStore;
use super::types::{
    Memory, MemoryConfig, MemoryMetadata, MemoryQuery, MemoryRelationship, MemorySearchResult,
    MemorySource, MemoryState, MemoryType, RelationshipType,
};
use crate::config::Config;
use crate::embedding::{create_embedding_provider_from_parts, parse_provider_model};

/// Parameters for the memorize() call — groups the optional fields to stay under clippy's arg limit.
#[derive(Debug)]
pub struct MemorizeParams {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub importance: Option<f32>,
    pub tags: Option<Vec<String>>,
    pub related_files: Option<Vec<String>>,
    pub source: Option<MemorySource>,
}
/// High-level memory management interface
pub struct MemoryManager {
    store: MemoryStore,
    config: MemoryConfig,
    /// Path to the stale-check marker file for incremental git scanning
    stale_check_marker: PathBuf,
    /// Path to the sleep-consolidation marker file; stores last-run RFC3339 timestamp.
    /// Lazy auto-consolidation is gated by `(now - last_run) >= interval_hours`.
    sleep_consolidation_marker: PathBuf,
}

impl MemoryManager {
    /// Create a new memory manager
    pub async fn new(
        config: &Config,
        project_key: Option<String>,
        role: Option<String>,
    ) -> Result<Self> {
        // Use memory config from main config (loaded from config file)
        let memory_config = config.memory.clone();

        // Create reranker integration if enabled
        let reranker_integration = if config.search.reranker.enabled {
            Some(
                crate::memory::reranker_integration::RerankerIntegration::new(
                    config.search.reranker.clone(),
                ),
            )
        } else {
            None
        };

        // Use shared memory database path (single DB for all projects)
        let db_path = crate::storage::get_memory_database_path()?;

        // Marker files: {db_dir}/.{kind}_{project_key}
        let project_label = project_key.as_deref().unwrap_or("default");
        let stale_check_marker = db_path.join(format!(".stale_check_{}", project_label));
        let sleep_consolidation_marker =
            db_path.join(format!(".sleep_consolidation_{}", project_label));

        // Create embedding provider using model from config
        let model_string = &config.embedding.model;
        let (provider, model) = parse_provider_model(model_string)?;
        let embedding_provider = create_embedding_provider_from_parts(&provider, &model).await?;

        let store = MemoryStore::new(
            db_path.to_string_lossy().as_ref(),
            project_key,
            role,
            embedding_provider,
            memory_config.clone(),
            config.clone(),
            reranker_integration,
        )
        .await?;

        let mut manager = Self {
            store,
            config: memory_config,
            stale_check_marker,
            sleep_consolidation_marker,
        };

        // Lazy cleanup of stale file references on init (like knowledge session cleanup)
        if manager.config.stale_ref_cleanup_enabled {
            manager.cleanup_stale_references().await.ok();
        }
        // Lazy autonomous sleep consolidation: marker-gated, no cron required.
        // Mirrors the cleanup pattern — best-effort, errors swallowed so a slow or
        // failed consolidation pass never blocks the manager from initializing.
        if manager.config.sleep_consolidation_enabled {
            manager.maybe_sleep_consolidate().await.ok();
        }

        Ok(manager)
    }

    /// Read the timestamp of the last sleep-consolidation pass from the marker file.
    fn read_sleep_marker(&self) -> Option<chrono::DateTime<Utc>> {
        let raw = std::fs::read_to_string(&self.sleep_consolidation_marker).ok()?;
        chrono::DateTime::parse_from_rfc3339(raw.trim())
            .ok()
            .map(|d| d.with_timezone(&Utc))
    }

    /// Write the current time as the last sleep-consolidation pass.
    fn write_sleep_marker(&self) {
        std::fs::write(&self.sleep_consolidation_marker, Utc::now().to_rfc3339()).ok();
    }

    /// Decide whether to run sleep consolidation based on the marker file.
    /// Runs if no marker exists OR `now - last_run >= interval_hours`.
    /// Always updates the marker on a successful run.
    async fn maybe_sleep_consolidate(&mut self) -> Result<()> {
        let interval_hours = self.config.sleep_consolidation_interval_hours.max(1) as i64;
        let due = match self.read_sleep_marker() {
            Some(last) => (Utc::now() - last).num_hours() >= interval_hours,
            None => true, // first run for this project
        };
        if !due {
            return Ok(());
        }

        let threshold = self.config.sleep_consolidation_threshold;
        let min_size = self.config.sleep_consolidation_min_cluster_size;
        let max_age_days = self.config.sleep_consolidation_max_age_days;

        let consolidated = self
            .sleep_consolidate(threshold, min_size, max_age_days)
            .await?;
        if !consolidated.is_empty() {
            tracing::info!(
                "Lazy sleep consolidation: produced {} consolidated parent(s)",
                consolidated.len()
            );
        }
        self.write_sleep_marker();
        Ok(())
    }

    /// Read the last commit we scanned for stale references.
    fn read_stale_check_marker(&self) -> Option<String> {
        std::fs::read_to_string(&self.stale_check_marker)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Write the current HEAD as the last scanned commit.
    fn write_stale_check_marker(&self, commit: &str) {
        std::fs::write(&self.stale_check_marker, commit).ok();
    }

    /// Clean up memories whose related_files no longer exist on disk.
    /// Git-aware: detects renames and auto-updates references instead of deleting.
    /// - Renamed files → update reference (self-healing)
    /// - ALL files deleted → delete the memory entirely
    /// - Some files deleted → remove dead paths, penalize importance
    ///   After cleanup, propagates staleness through the relationship graph.
    ///   Incremental: tracks the last scanned commit in a marker file.
    /// - HEAD unchanged since last check → skip entirely
    /// - HEAD advanced → scan only the delta (last_checked..HEAD)
    /// - First run → scan from oldest memory's commit
    async fn cleanup_stale_references(&mut self) -> Result<usize> {
        // Without a project key we cannot determine which git repo to check
        // file existence against — skip entirely to avoid deleting memories
        // from unrelated projects.
        if self.store.has_no_project_key() {
            return Ok(0);
        }

        // Determine scan range
        let current_head = match GitUtils::get_current_commit() {
            Some(h) => h,
            None => return Ok(0), // not in a git repo
        };

        let last_checked = self.read_stale_check_marker();
        if last_checked.as_deref() == Some(current_head.as_str()) {
            return Ok(0); // HEAD unchanged — nothing to do
        }

        let memories = self.store.get_memories_with_files().await?;
        if memories.is_empty() {
            self.write_stale_check_marker(&current_head);
            return Ok(0);
        }

        // Build rename map for the scan range (single git call)
        let scan_from = match &last_checked {
            Some(commit) => commit.as_str(),
            None => {
                // First run: find oldest memory's commit
                memories
                    .iter()
                    .filter_map(|m| {
                        m.metadata
                            .git_commit
                            .as_deref()
                            .filter(|c| !c.is_empty())
                            .map(|c| (m.created_at, c))
                    })
                    .min_by_key(|(created_at, _)| *created_at)
                    .map(|(_, commit)| commit)
                    .unwrap_or(&current_head)
            }
        };
        let rename_map = RenameMap::build(scan_from);

        let mut cleaned = 0;
        let mut affected_ids: Vec<String> = Vec::new();

        for memory in &memories {
            if memory.metadata.related_files.is_empty() {
                continue;
            }

            let mut alive: Vec<String> = Vec::new();
            let mut dead_count: usize = 0;
            let mut renamed = false;

            for file in &memory.metadata.related_files {
                match GitUtils::check_file_fate(file, Some(&rename_map)) {
                    FileFate::Exists => alive.push(file.clone()),
                    FileFate::Renamed(new_path) => {
                        tracing::info!(
                            "Memory '{}': file '{}' renamed to '{}'",
                            memory.title,
                            file,
                            new_path
                        );
                        alive.push(new_path);
                        renamed = true;
                    }
                    FileFate::Deleted => dead_count += 1,
                    FileFate::Unknown => {
                        if GitUtils::file_exists(file) {
                            alive.push(file.clone());
                        } else {
                            dead_count += 1;
                        }
                    }
                }
            }

            if dead_count == 0 && !renamed {
                continue;
            }

            if alive.is_empty() {
                self.store.delete_memory(&memory.id).await?;
                affected_ids.push(memory.id.clone());
                tracing::info!(
                    "Deleted stale memory '{}' — all related files are gone",
                    memory.title,
                );
            } else {
                let mut updated = memory.clone();
                updated.metadata.related_files = alive;

                if dead_count > 0 {
                    let penalty = self
                        .config
                        .stale_ref_importance_penalty
                        .powi(dead_count as i32);
                    updated.metadata.importance = (updated.metadata.importance * penalty)
                        .max(self.config.min_importance_threshold);
                    affected_ids.push(memory.id.clone());
                }

                self.store.update_memory(&updated).await?;
                tracing::info!(
                    "Updated memory '{}' — {} renamed, {} dead, importance {:.2}",
                    updated.title,
                    if renamed { "some" } else { "none" },
                    dead_count,
                    updated.metadata.importance
                );
            }

            cleaned += 1;
        }

        // Propagate staleness through the relationship graph (1 hop)
        if !affected_ids.is_empty() {
            let propagated = self.propagate_staleness(&affected_ids).await?;
            if propagated > 0 {
                tracing::info!(
                    "Staleness propagation: penalized {} related memories",
                    propagated
                );
            }
        }

        if cleaned > 0 {
            tracing::info!("Stale reference cleanup: processed {} memories", cleaned);
        }

        // Mark this HEAD as scanned
        self.write_stale_check_marker(&current_head);

        Ok(cleaned)
    }

    /// Propagate importance penalties through relationships (1 hop).
    /// DependsOn → 0.5x penalty, RelatedTo/AutoLinked → 0.9x penalty.
    async fn propagate_staleness(&mut self, stale_ids: &[String]) -> Result<usize> {
        let mut penalized = 0;
        let mut seen = std::collections::HashSet::new();

        for stale_id in stale_ids {
            let rels = self.store.get_memory_relationships(stale_id).await?;
            for rel in rels {
                // Find the neighbor (the other end of the relationship)
                let neighbor_id = if rel.source_id == *stale_id {
                    &rel.target_id
                } else {
                    &rel.source_id
                };

                // Skip if already penalized or if it's also stale
                if seen.contains(neighbor_id) || stale_ids.contains(neighbor_id) {
                    continue;
                }
                seen.insert(neighbor_id.clone());

                let penalty = match rel.relationship_type {
                    RelationshipType::DependsOn => 0.5,
                    _ => 0.9, // RelatedTo, AutoLinked, Similar, etc.
                };

                if let Some(mut neighbor) = self.store.get_memory(neighbor_id).await? {
                    neighbor.metadata.importance = (neighbor.metadata.importance * penalty)
                        .max(self.config.min_importance_threshold);
                    self.store.update_memory(&neighbor).await?;
                    penalized += 1;
                }
            }
        }

        Ok(penalized)
    }

    /// Memorize new information with automatic Git context
    pub async fn memorize(&mut self, params: MemorizeParams) -> Result<Memory> {
        let MemorizeParams {
            memory_type,
            title,
            content,
            importance,
            tags,
            related_files,
            source,
        } = params;

        // Initialize metadata with all values at once to satisfy clippy
        let mut metadata = MemoryMetadata {
            git_commit: GitUtils::get_current_commit(),
            importance: importance.unwrap_or(self.config.default_importance),
            tags: tags.unwrap_or_default(),
            related_files: Vec::new(),
            source: source.unwrap_or_default(),
            ..Default::default()
        };

        // Add related files (convert to relative paths if possible)
        if let Some(files) = related_files {
            metadata.related_files = files
                .into_iter()
                .map(|file| GitUtils::get_relative_path(&file).unwrap_or(file))
                .collect();
        }

        // Auto-detect related files from Git changes if none provided
        if metadata.related_files.is_empty() {
            if let Ok(modified_files) = GitUtils::get_modified_files() {
                metadata.related_files = modified_files.into_iter().take(5).collect();
            }
        }

        let memory = Memory::new(memory_type, title, content, Some(metadata));

        // Store the memory
        self.store.store_memory(&memory).await?;

        // Auto-link to similar memories and file-sharing memories if enabled
        if self.config.auto_linking_enabled {
            self.auto_link_memory(&memory.id).await?;
        }

        Ok(memory)
    }

    /// Remember (search) memories based on query
    pub async fn remember(
        &self,
        query: &str,
        filters: Option<MemoryQuery>,
    ) -> Result<Vec<MemorySearchResult>> {
        let mut search_query = filters.unwrap_or_default();
        search_query.query_text = Some(query.to_string());

        self.store.search_memories(&search_query).await
    }

    /// Remember (search) memories based on multiple queries with relevance-based merging
    pub async fn remember_multi(
        &self,
        queries: &[String],
        filters: Option<MemoryQuery>,
    ) -> Result<Vec<MemorySearchResult>> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }

        if queries.len() == 1 {
            // Single query - use existing method
            return self.remember(&queries[0], filters).await;
        }

        // Multiple queries - search each and merge results by relevance
        let base_filters = filters.unwrap_or_default();
        let mut all_results: std::collections::HashMap<String, MemorySearchResult> =
            std::collections::HashMap::new();
        let mut query_count: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        // Search with each query
        for query in queries {
            let mut search_query = base_filters.clone();
            search_query.query_text = Some(query.clone());

            let results = self.store.search_memories(&search_query).await?;

            for result in results {
                let memory_id = result.memory.id.clone();

                // Track how many queries matched this memory
                *query_count.entry(memory_id.clone()).or_insert(0) += 1;

                // Keep the result with highest relevance score
                match all_results.get(&memory_id) {
                    Some(existing) if existing.relevance_score >= result.relevance_score => {
                        // Keep existing with higher score
                    }
                    _ => {
                        // Use this result (higher score or first occurrence)
                        all_results.insert(memory_id, result);
                    }
                }
            }
        }

        // Convert to vector and boost scores for memories that matched multiple queries
        let mut final_results: Vec<MemorySearchResult> = all_results
            .into_iter()
            .map(|(memory_id, mut result)| {
                let matches = query_count.get(&memory_id).unwrap_or(&1);

                // Boost relevance score for memories matching multiple queries
                if *matches > 1 {
                    let boost_factor = 1.0 + ((*matches as f32 - 1.0) * 0.1); // 10% boost per additional match
                    result.relevance_score = (result.relevance_score * boost_factor).min(1.0);

                    // Update selection reason to indicate multi-query match
                    result.selection_reason = format!(
                        "Matched {} of {} queries: {}",
                        matches,
                        queries.len(),
                        result.selection_reason
                    );
                }

                result
            })
            .collect();

        // Sort by relevance score (highest first)
        final_results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply limit if specified in filters
        if let Some(limit) = base_filters.limit {
            final_results.truncate(limit);
        }

        Ok(final_results)
    }

    /// Forget (delete) a memory by ID
    pub async fn forget(&mut self, memory_id: &str) -> Result<()> {
        self.store.delete_memory(memory_id).await
    }

    /// Forget memories matching criteria
    pub async fn forget_matching(&mut self, query: MemoryQuery) -> Result<usize> {
        let search_results = self.store.search_memories(&query).await?;
        let mut deleted_count = 0;

        for result in search_results {
            self.store.delete_memory(&result.memory.id).await?;
            deleted_count += 1;
        }

        Ok(deleted_count)
    }
    /// Update an existing memory
    pub async fn update_memory(
        &mut self,
        memory_id: &str,
        title: Option<String>,
        content: Option<String>,
        metadata_updates: Option<MemoryMetadata>,
    ) -> Result<Option<Memory>> {
        if let Some(mut memory) = self.store.get_memory(memory_id).await? {
            // Update Git commit to current
            let current_commit = GitUtils::get_current_commit();
            if let Some(mut meta) = metadata_updates {
                meta.git_commit = current_commit.clone();
                memory.update(title, content, Some(meta));
            } else if let Some(commit) = current_commit {
                memory.metadata.git_commit = Some(commit);
                memory.update(title, content, None);
            } else {
                memory.update(title, content, None);
            }

            self.store.update_memory(&memory).await?;

            // Re-link: clear old AutoLinked rels then rebuild with updated content/files
            if self.config.auto_linking_enabled {
                self.store
                    .delete_auto_linked_relationships(memory_id)
                    .await?;
                self.auto_link_memory(memory_id).await?;
            }

            Ok(Some(memory))
        } else {
            Ok(None)
        }
    }

    /// Get memory by ID
    pub async fn get_memory(&self, memory_id: &str) -> Result<Option<Memory>> {
        self.store.get_memory(memory_id).await
    }

    /// Get recent memories
    pub async fn get_recent_memories(&self, limit: usize) -> Result<Vec<Memory>> {
        let query = MemoryQuery {
            limit: Some(limit),
            sort_by: Some(super::types::MemorySortBy::CreatedAt),
            sort_order: Some(super::types::SortOrder::Descending),
            ..Default::default()
        };

        let results = self.store.search_memories(&query).await?;
        Ok(results.into_iter().map(|r| r.memory).collect())
    }

    /// Get memories by type
    pub async fn get_memories_by_type(
        &self,
        memory_type: MemoryType,
        limit: Option<usize>,
    ) -> Result<Vec<Memory>> {
        let query = MemoryQuery {
            memory_types: Some(vec![memory_type]),
            limit,
            sort_by: Some(super::types::MemorySortBy::CreatedAt),
            sort_order: Some(super::types::SortOrder::Descending),
            ..Default::default()
        };

        let results = self.store.search_memories(&query).await?;
        Ok(results.into_iter().map(|r| r.memory).collect())
    }

    /// Get memories related to files
    pub async fn get_memories_for_files(
        &self,
        file_paths: Vec<String>,
    ) -> Result<Vec<MemorySearchResult>> {
        // Convert to relative paths
        let relative_paths: Vec<String> = file_paths
            .into_iter()
            .map(|path| GitUtils::get_relative_path(&path).unwrap_or(path))
            .collect();

        let query = MemoryQuery {
            related_files: Some(relative_paths),
            sort_by: Some(super::types::MemorySortBy::Importance),
            sort_order: Some(super::types::SortOrder::Descending),
            ..Default::default()
        };

        self.store.search_memories(&query).await
    }

    /// Get memories for current Git commit
    pub async fn get_memories_for_current_commit(&self) -> Result<Vec<Memory>> {
        if let Some(commit) = GitUtils::get_current_commit() {
            let query = MemoryQuery {
                git_commit: Some(commit),
                sort_by: Some(super::types::MemorySortBy::CreatedAt),
                sort_order: Some(super::types::SortOrder::Descending),
                ..Default::default()
            };

            let results = self.store.search_memories(&query).await?;
            Ok(results.into_iter().map(|r| r.memory).collect())
        } else {
            Ok(Vec::new())
        }
    }

    /// Get memories with tags
    pub async fn get_memories_by_tags(&self, tags: Vec<String>) -> Result<Vec<MemorySearchResult>> {
        let query = MemoryQuery {
            tags: Some(tags),
            sort_by: Some(super::types::MemorySortBy::Importance),
            sort_order: Some(super::types::SortOrder::Descending),
            ..Default::default()
        };

        self.store.search_memories(&query).await
    }

    /// Get memory statistics
    pub async fn get_memory_stats(&self) -> Result<MemoryStats> {
        let total_count = self.store.get_memory_count().await?;

        // Get count by type (simplified - would need custom queries for exact counts)
        let recent_memories = self.get_recent_memories(100).await?;
        let mut type_counts = std::collections::HashMap::new();

        for memory in &recent_memories {
            *type_counts
                .entry(memory.memory_type.to_string())
                .or_insert(0) += 1;
        }

        let (projects, roles) = self.store.get_distinct_projects_and_roles().await?;

        Ok(MemoryStats {
            total_memories: total_count,
            type_counts,
            recent_count: recent_memories.len().min(10),
            git_commit: GitUtils::get_current_commit(),
            projects,
            roles,
        })
    }

    /// Create a relationship between two memories
    pub async fn create_relationship(
        &mut self,
        source_id: String,
        target_id: String,
        relationship_type: RelationshipType,
        strength: f32,
        description: String,
    ) -> Result<MemoryRelationship> {
        let relationship = MemoryRelationship {
            id: uuid::Uuid::new_v4().to_string(),
            source_id,
            target_id,
            relationship_type,
            strength,
            description,
            created_at: Utc::now(),
        };

        self.store.store_relationship(&relationship).await?;
        Ok(relationship)
    }

    /// Get relationships for a memory
    pub async fn get_relationships(&self, memory_id: &str) -> Result<Vec<MemoryRelationship>> {
        self.store.get_memory_relationships(memory_id).await
    }

    /// Get related memories through relationships
    pub async fn get_related_memories(&self, memory_id: &str) -> Result<Vec<Memory>> {
        let relationships = self.get_relationships(memory_id).await?;
        let mut related_memories = Vec::new();

        for rel in relationships {
            let related_id = if rel.source_id == memory_id {
                rel.target_id
            } else {
                rel.source_id
            };

            if let Some(memory) = self.store.get_memory(&related_id).await? {
                related_memories.push(memory);
            }
        }

        Ok(related_memories)
    }

    /// Event-based consolidation: close a Goal memory by summarizing all source
    /// memories that `Achieves` it into a new consolidated parent.
    ///
    /// Flow:
    /// 1. Validate the goal exists and is of type Goal
    /// 2. Find all memories with an `Achieves` relationship targeting this goal
    /// 3. Synthesize a consolidated memory (importance = max(sources) * 1.1, clamped)
    /// 4. Link consolidated → goal with `Closes`; link consolidated → each source with `AutoLinked`
    /// 5. Transition each source to `MemoryState::Consolidated` with importance × 0.2
    ///    via a partial column UPDATE (embedding untouched)
    ///
    /// `summary` lets the caller supply an LLM-generated summary; if `None` the
    /// consolidated memory's content is a deterministic synthesis of source titles.
    pub async fn consolidate_goal(
        &mut self,
        goal_id: &str,
        summary: Option<String>,
    ) -> Result<Memory> {
        let goal = self
            .store
            .get_memory(goal_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Goal memory '{}' not found", goal_id))?;
        if goal.memory_type != MemoryType::Goal {
            return Err(anyhow::anyhow!(
                "Memory '{}' is type {} — consolidate_goal requires MemoryType::Goal",
                goal_id,
                goal.memory_type
            ));
        }

        // Find Achieves relationships targeting this goal (source → goal).
        let achievers: Vec<MemoryRelationship> = self
            .store
            .get_memory_relationships(goal_id)
            .await?
            .into_iter()
            .filter(|r| {
                matches!(r.relationship_type, RelationshipType::Achieves) && r.target_id == goal_id
            })
            .collect();

        if achievers.is_empty() {
            return Err(anyhow::anyhow!(
                "Goal '{}' has no Achieves source memories to consolidate",
                goal_id
            ));
        }

        // Load the source memories. Skip any that have already been consolidated
        // (idempotent: re-running consolidate_goal won't re-process archived sources).
        let mut sources: Vec<Memory> = Vec::with_capacity(achievers.len());
        for rel in &achievers {
            if let Some(m) = self.store.get_memory(&rel.source_id).await? {
                if m.metadata.state == MemoryState::Working {
                    sources.push(m);
                }
            }
        }
        if sources.is_empty() {
            return Err(anyhow::anyhow!(
                "Goal '{}' has Achieves relationships but no Working source memories",
                goal_id
            ));
        }

        // Consolidated importance: 10% above the strongest source, clamped to [0,1].
        let max_src_importance: f32 = sources
            .iter()
            .map(|m| m.metadata.importance)
            .fold(0.0_f32, f32::max);
        let consolidated_importance = (max_src_importance * 1.1).clamp(0.0, 1.0);

        let consolidated_content = summary.unwrap_or_else(|| {
            let titles: Vec<&str> = sources.iter().map(|m| m.title.as_str()).collect();
            format!(
                "Consolidation of goal '{}' — synthesized from {} source memories:\n- {}",
                goal.title,
                sources.len(),
                titles.join("\n- ")
            )
        });

        let mut consolidated_meta = MemoryMetadata {
            importance: consolidated_importance,
            source: goal.metadata.source.clone(),
            ..Default::default()
        };
        consolidated_meta.tags.push("consolidated".to_string());
        consolidated_meta.tags.push(format!("goal:{}", goal_id));
        consolidated_meta.decay = super::types::MemoryDecay::new(consolidated_importance);

        let consolidated = Memory::new(
            MemoryType::Insight,
            format!("Consolidation: {}", goal.title),
            consolidated_content,
            Some(consolidated_meta),
        );
        self.store.store_memory(&consolidated).await?;

        // Close-relationship marks the consolidation event.
        let closes = MemoryRelationship {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: consolidated.id.clone(),
            target_id: goal_id.to_string(),
            relationship_type: RelationshipType::Closes,
            strength: 1.0,
            description: format!("Closes goal via consolidation of {} sources", sources.len()),
            created_at: Utc::now(),
        };
        self.store.store_relationship(&closes).await?;

        // Provenance: link consolidated → each source so the chain is queryable.
        for src in &sources {
            let link = MemoryRelationship {
                id: uuid::Uuid::new_v4().to_string(),
                source_id: consolidated.id.clone(),
                target_id: src.id.clone(),
                relationship_type: RelationshipType::AutoLinked,
                strength: 0.9,
                description: "Source absorbed by consolidation".to_string(),
                created_at: Utc::now(),
            };
            self.store.store_relationship(&link).await?;
        }

        // Archive sources: dampen importance, transition state — partial UPDATE,
        // no embedding regen, no full row rewrite.
        for src in &sources {
            let new_importance = src.metadata.importance * 0.2;
            self.store
                .update_state_and_importance(&src.id, MemoryState::Consolidated, new_importance)
                .await?;
        }

        tracing::info!(
            "Consolidated goal '{}' ({}): {} sources → new memory {} (importance={:.3})",
            goal.title,
            goal_id,
            sources.len(),
            consolidated.id,
            consolidated_importance,
        );

        Ok(consolidated)
    }

    /// Sleep consolidation: batch-find clusters of similar recent memories and
    /// fold each cluster into a consolidated parent via the same goal-anchored
    /// pipeline `consolidate_goal` uses.
    ///
    /// Process:
    /// 1. Fetch all Working-state memories created in the last `max_age_days`
    /// 2. For each candidate (in order), search for similar candidates above
    ///    `similarity_threshold` and form a cluster {candidate} ∪ neighbors,
    ///    excluding anything already assigned to another cluster
    /// 3. For each cluster of size ≥ `min_cluster_size`, synthesize an
    ///    ephemeral `Goal` memory, link cluster members via `Achieves`, then
    ///    call `consolidate_goal`. The synthetic goal becomes part of the
    ///    permanent provenance chain — no special teardown needed.
    ///
    /// Returns the consolidated memories produced. Empty vec when nothing
    /// clusters tightly enough at the given threshold.
    pub async fn sleep_consolidate(
        &mut self,
        similarity_threshold: f32,
        min_cluster_size: usize,
        max_age_days: u32,
    ) -> Result<Vec<Memory>> {
        if min_cluster_size < 2 {
            return Err(anyhow::anyhow!(
                "min_cluster_size must be >= 2, got {}",
                min_cluster_size
            ));
        }

        let cutoff = Utc::now() - Duration::days(max_age_days as i64);
        let candidates = self.store.get_recent_working_memories(cutoff).await?;
        if candidates.len() < min_cluster_size {
            return Ok(Vec::new());
        }

        // For each candidate, find similar Working memories above threshold.
        // We collect (id, neighbor_ids) pairs then run pure clustering on them.
        let candidate_ids: HashSet<String> = candidates.iter().map(|m| m.id.clone()).collect();
        let mut neighborhoods: Vec<(String, Vec<String>)> = Vec::with_capacity(candidates.len());
        for cand in &candidates {
            let query = MemoryQuery {
                query_text: Some(cand.get_searchable_text()),
                limit: Some(self.config.max_auto_links_per_memory.max(min_cluster_size) * 2),
                min_relevance: Some(similarity_threshold),
                ..Default::default()
            };
            let hits = self.store.search_memories(&query).await?;
            let neighbors: Vec<String> = hits
                .into_iter()
                .filter(|r| r.memory.id != cand.id && candidate_ids.contains(&r.memory.id))
                .map(|r| r.memory.id)
                .collect();
            neighborhoods.push((cand.id.clone(), neighbors));
        }

        let clusters = build_clusters(&neighborhoods, min_cluster_size);
        if clusters.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now();
        let mut consolidated = Vec::with_capacity(clusters.len());
        for cluster in clusters {
            // Synthesize an ephemeral Goal so we can reuse consolidate_goal verbatim.
            let goal_meta = MemoryMetadata {
                importance: 0.5,
                source: MemorySource::AgentInferred,
                ..Default::default()
            };
            let goal = Memory::new(
                MemoryType::Goal,
                format!("Sleep cluster {}", now.format("%Y-%m-%d %H:%M:%S")),
                format!(
                    "Auto-detected cluster of {} similar memories created in the last {} days, \
                     similarity ≥ {:.2}",
                    cluster.len(),
                    max_age_days,
                    similarity_threshold
                ),
                Some(goal_meta),
            );
            self.store.store_memory(&goal).await?;

            for member_id in &cluster {
                let achieves = MemoryRelationship {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id: member_id.clone(),
                    target_id: goal.id.clone(),
                    relationship_type: RelationshipType::Achieves,
                    strength: 1.0,
                    description: "Sleep consolidation cluster member".to_string(),
                    created_at: now,
                };
                self.store.store_relationship(&achieves).await?;
            }

            match self.consolidate_goal(&goal.id, None).await {
                Ok(m) => consolidated.push(m),
                Err(e) => tracing::warn!(
                    "Sleep consolidation: cluster {:?} failed to consolidate: {}",
                    cluster,
                    e
                ),
            }
        }

        Ok(consolidated)
    }

    /// Automatically link a memory to similar memories based on semantic similarity
    /// Creates bidirectional AutoLinked relationships
    pub async fn auto_link_memory(&mut self, memory_id: &str) -> Result<Vec<MemoryRelationship>> {
        if !self.config.auto_linking_enabled {
            return Ok(Vec::new());
        }

        // 1. Get the memory
        let memory = self
            .store
            .get_memory(memory_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Memory not found: {}", memory_id))?;

        // 2. Search for similar memories with high threshold
        let query = MemoryQuery {
            query_text: Some(memory.get_searchable_text()),
            limit: Some(self.config.max_auto_links_per_memory * 2), // Get more candidates
            min_relevance: Some(self.config.auto_link_threshold),
            ..Default::default()
        };

        let similar = self.store.search_memories(&query).await?;

        // 3. Create bidirectional similarity relationships
        let mut relationships = Vec::new();
        let mut link_count = 0;

        for result in similar.iter() {
            // Skip self-linking
            if result.memory.id == memory_id {
                continue;
            }

            // Stop if we've reached max links
            if link_count >= self.config.max_auto_links_per_memory {
                break;
            }

            // Create forward link (source -> target)
            let forward_rel = MemoryRelationship {
                id: uuid::Uuid::new_v4().to_string(),
                source_id: memory_id.to_string(),
                target_id: result.memory.id.clone(),
                relationship_type: RelationshipType::AutoLinked,
                strength: result.relevance_score,
                description: format!("Auto-linked (similarity: {:.2})", result.relevance_score),
                created_at: Utc::now(),
            };

            self.store.store_relationship(&forward_rel).await?;
            relationships.push(forward_rel);

            // Create backward link if bidirectional (target -> source)
            if self.config.bidirectional_links {
                let backward_rel = MemoryRelationship {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id: result.memory.id.clone(),
                    target_id: memory_id.to_string(),
                    relationship_type: RelationshipType::AutoLinked,
                    strength: result.relevance_score,
                    description: format!("Auto-linked (similarity: {:.2})", result.relevance_score),
                    created_at: Utc::now(),
                };

                self.store.store_relationship(&backward_rel).await?;
                relationships.push(backward_rel);
            }

            link_count += 1;
        }

        // 4. File-based relationships: link memories that share related files
        //    Only consider files that still exist on disk to avoid linking via dead references
        let live_files: Vec<String> = memory
            .metadata
            .related_files
            .iter()
            .filter(|f| GitUtils::file_exists(f))
            .cloned()
            .collect();
        if !live_files.is_empty() {
            let file_query = MemoryQuery {
                related_files: Some(live_files),
                limit: Some(10),
                ..Default::default()
            };

            let file_related = self.store.search_memories(&file_query).await?;
            for result in file_related {
                if result.memory.id == memory_id {
                    continue;
                }
                // Skip if already linked by similarity pass
                if relationships.iter().any(|r: &MemoryRelationship| {
                    r.target_id == result.memory.id || r.source_id == result.memory.id
                }) {
                    continue;
                }

                let file_rel = MemoryRelationship {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id: memory_id.to_string(),
                    target_id: result.memory.id.clone(),
                    relationship_type: RelationshipType::RelatedTo,
                    strength: 0.7,
                    description: "Shares related files".to_string(),
                    created_at: Utc::now(),
                };
                self.store.store_relationship(&file_rel).await?;
                relationships.push(file_rel);
            }
        }

        Ok(relationships)
    }

    /// Get memory graph starting from a memory ID with specified depth
    /// Uses BFS to traverse relationships and build a graph
    pub async fn get_memory_graph(
        &self,
        memory_id: &str,
        depth: usize,
    ) -> Result<super::types::MemoryGraph> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let mut graph = super::types::MemoryGraph {
            root: memory_id.to_string(),
            memories: HashMap::new(),
            relationships: Vec::new(),
        };

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back((memory_id.to_string(), 0));

        while let Some((current_id, current_depth)) = queue.pop_front() {
            // Skip if already visited or exceeded depth
            if visited.contains(&current_id) || current_depth > depth {
                continue;
            }

            visited.insert(current_id.clone());

            // Get memory
            if let Some(memory) = self.store.get_memory(&current_id).await? {
                graph.memories.insert(current_id.clone(), memory);
            }

            // Get relationships
            let rels = self.store.get_memory_relationships(&current_id).await?;

            // Add relationships to graph (avoid duplicates)
            for rel in &rels {
                if !graph.relationships.iter().any(|r| r.id == rel.id) {
                    graph.relationships.push(rel.clone());
                }
            }

            // Add connected memories to queue if within depth
            if current_depth < depth {
                for rel in rels {
                    // Add both source and target to explore bidirectional links
                    if rel.source_id == current_id && !visited.contains(&rel.target_id) {
                        queue.push_back((rel.target_id, current_depth + 1));
                    } else if rel.target_id == current_id && !visited.contains(&rel.source_id) {
                        queue.push_back((rel.source_id, current_depth + 1));
                    }
                }
            }
        }

        Ok(graph)
    }

    /// Clean up old memories and stale file references
    pub async fn cleanup(&mut self) -> Result<usize> {
        let mut total = self.store.cleanup_old_memories().await?;
        if self.config.stale_ref_cleanup_enabled {
            total += self.cleanup_stale_references().await?;
        }
        Ok(total)
    }

    /// Clear all memory data (DANGEROUS: deletes all memories and relationships)
    pub async fn clear_all(&mut self) -> Result<usize> {
        self.store.clear_all_memory_data().await
    }

    /// Add tag to memory
    pub async fn add_tag(&mut self, memory_id: &str, tag: String) -> Result<bool> {
        if let Some(mut memory) = self.store.get_memory(memory_id).await? {
            memory.add_tag(tag);
            self.store.update_memory(&memory).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove tag from memory
    pub async fn remove_tag(&mut self, memory_id: &str, tag: &str) -> Result<bool> {
        if let Some(mut memory) = self.store.get_memory(memory_id).await? {
            memory.remove_tag(tag);
            self.store.update_memory(&memory).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Add related file to memory
    pub async fn add_related_file(&mut self, memory_id: &str, file_path: String) -> Result<bool> {
        if let Some(mut memory) = self.store.get_memory(memory_id).await? {
            let relative_path = GitUtils::get_relative_path(&file_path).unwrap_or(file_path);
            memory.add_related_file(relative_path);
            self.store.update_memory(&memory).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove related file from memory
    pub async fn remove_related_file(&mut self, memory_id: &str, file_path: &str) -> Result<bool> {
        if let Some(mut memory) = self.store.get_memory(memory_id).await? {
            memory.remove_related_file(file_path);
            self.store.update_memory(&memory).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Enable reranker with optional model override
    pub fn enable_reranker(&mut self, model: Option<String>) {
        self.store.enable_reranker(model);
    }

    /// Disable reranker
    pub fn disable_reranker(&mut self) {
        self.store.disable_reranker();
    }
}

/// Greedy clustering on a candidate / neighbors list.
///
/// Each candidate is visited in input order. If it hasn't already been claimed
/// by an earlier cluster, it forms a new candidate cluster = {self} ∪ {unclaimed
/// neighbors}. Clusters smaller than `min_size` are discarded. Members claimed
/// by an accepted cluster cannot join later ones — output clusters are disjoint.
///
/// Pure function so it can be unit-tested without LanceDB.
pub(crate) fn build_clusters(
    candidates: &[(String, Vec<String>)],
    min_size: usize,
) -> Vec<Vec<String>> {
    let mut claimed: HashSet<String> = HashSet::new();
    let mut clusters: Vec<Vec<String>> = Vec::new();

    for (id, neighbors) in candidates {
        if claimed.contains(id) {
            continue;
        }
        let mut cluster: Vec<String> = vec![id.clone()];
        for n in neighbors {
            if n != id && !claimed.contains(n) && !cluster.contains(n) {
                cluster.push(n.clone());
            }
        }
        if cluster.len() >= min_size {
            for m in &cluster {
                claimed.insert(m.clone());
            }
            clusters.push(cluster);
        }
    }

    clusters
}

/// Memory statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_memories: usize,
    pub type_counts: std::collections::HashMap<String, usize>,
    pub recent_count: usize,
    pub git_commit: Option<String>,
    pub projects: Vec<String>,
    pub roles: Vec<String>,
}

impl MemoryStats {
    /// Format stats as human-readable string
    pub fn format(&self) -> String {
        let mut output = "Memory Statistics:\n".to_string();
        output.push_str(&format!("  Total memories: {}\n", self.total_memories));
        output.push_str(&format!("  Recent memories: {}\n", self.recent_count));

        if let Some(ref commit) = self.git_commit {
            output.push_str(&format!("  Current commit: {}\n", commit));
        }

        if !self.projects.is_empty() {
            output.push_str("  Projects:\n");
            for project in &self.projects {
                output.push_str(&format!("    {}\n", project));
            }
        }

        if !self.roles.is_empty() {
            output.push_str("  Roles:\n");
            for role in &self.roles {
                output.push_str(&format!("    {}\n", role));
            }
        }

        if !self.type_counts.is_empty() {
            output.push_str("  Memory types:\n");
            let mut types: Vec<_> = self.type_counts.iter().collect();
            types.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (memory_type, count) in types {
                output.push_str(&format!("    {}: {}\n", memory_type, count));
            }
        }

        output
    }
}
