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

use std::path::Path;

use anyhow::{Context, Result};
use octolib::utils::config_file;
use octolib::utils::config_migration::MigrationPlan;
use serde::{Deserialize, Serialize};

use crate::memory::types::MemoryConfig;

const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../config-templates/default.toml");

/// Releases before the `version` stamp existed shipped what is now v1, so a
/// config with no version is v1 rather than a broken file.
const LEGACY_CONFIG_VERSION: u32 = 1;

/// Octobrain's version chain. The driver (version walk, guards, table merging)
/// and the file primitives (lock, backup, atomic replace) live in octolib;
/// only the per-version steps belong here.
///
/// Add a `VersionMigration { from: N, to: N + 1, apply }` here in the same
/// commit that bumps `version` in `config-templates/default.toml` — the driver
/// then walks v1 -> v2 -> v3 in order on its own.
fn plan() -> MigrationPlan {
    MigrationPlan::new("octobrain", Vec::new()).with_missing_version(LEGACY_CONFIG_VERSION)
}

/// Embedding configuration for memory operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub model: String,
    pub batch_size: usize,
    pub max_tokens_per_batch: usize,
    /// Timeout in seconds for embedding generation calls (0 = disabled)
    pub timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "fastembed:BAAI/bge-small-en-v1.5".to_string(),
            batch_size: 32,
            max_tokens_per_batch: 100000,
            timeout_secs: 30,
        }
    }
}

/// Search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub similarity_threshold: f32,
    pub max_results: usize,
    /// Hybrid search configuration
    pub hybrid: HybridSearchConfig,
    /// Reranker configuration for improving search accuracy
    pub reranker: RerankerConfig,
    /// Pseudo-relevance feedback (PRF / HyDE-lite) query expansion
    pub hyde: HydeConfig,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.3,
            max_results: 50,
            hybrid: HybridSearchConfig::default(),
            reranker: RerankerConfig {
                enabled: false,
                model: "voyage:rerank-2.5".to_string(),
                top_k_candidates: 50,
                final_top_k: 10,
                timeout_secs: 30,
            },
            hyde: HydeConfig::default(),
        }
    }
}

/// Pseudo-relevance feedback query expansion (Rocchio-style centroid blending).
///
/// When enabled, every query runs a cheap first-pass vector retrieval, takes the
/// centroid of the top-K embeddings, and blends it with the original query:
/// `expanded = alpha * original + (1 - alpha) * centroid`. The expanded vector is
/// then used for the actual search. Costs one extra LanceDB vector query per search
/// in exchange for typically +10-30% recall on long-tail queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydeConfig {
    pub enabled: bool,
    /// Number of nearest neighbors to average for the centroid.
    pub top_k: usize,
    /// Blend weight on the original query embedding. 1.0 = no expansion, 0.0 = full centroid replacement.
    pub alpha: f32,
}

impl Default for HydeConfig {
    fn default() -> Self {
        Self {
            // Default ON: autonomous improvement, no LLM dependency. Costs one
            // extra LanceDB vector query per search; lifts long-tail recall.
            enabled: true,
            top_k: 3,
            alpha: 0.5,
        }
    }
}

/// Hybrid search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchConfig {
    /// Enable hybrid search (native BM25 + vector RRF fusion via LanceDB)
    pub enabled: bool,
    /// Weight applied to the RRF-fused score (vector + BM25 combined)
    pub default_vector_weight: f32,
    /// Default weight for recency signal
    pub default_recency_weight: f32,
    /// Default weight for importance signal
    pub default_importance_weight: f32,
    /// Recency decay period in days
    pub recency_decay_days: u32,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_vector_weight: 0.8,
            default_recency_weight: 0.1,
            default_importance_weight: 0.1,
            recency_decay_days: 30,
        }
    }
}

/// Knowledge base configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub outdating_days: u64,
    pub max_results: usize,
    /// Hours after which session-scoped chunks are cleaned up (crash recovery)
    pub session_ttl_hours: u64,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            chunk_size: 1200,
            chunk_overlap: 300,
            outdating_days: 15,
            max_results: 5,
            session_ttl_hours: 120,
        }
    }
}

/// Main configuration for octobrain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "legacy_config_version")]
    pub version: u32,
    pub embedding: EmbeddingConfig,
    pub search: SearchConfig,
    pub memory: MemoryConfig,
    pub knowledge: KnowledgeConfig,
}
impl Config {
    /// Load configuration from config.toml file
    /// Creates missing configs from the template embedded in this binary and migrates
    /// older configs one version at a time.
    /// STRICT: All config fields must be explicitly defined - no defaults allowed
    pub fn load() -> Result<Self> {
        let config_path = crate::storage::get_config_path()?;
        Self::load_from_path(&config_path)
    }

    fn load_from_path(config_path: &Path) -> Result<Self> {
        // Fast path: a current (or absent-but-creatable) config must not take the
        // write lock. Only an actual migration serialises.
        if config_path.exists() {
            let original = std::fs::read_to_string(config_path)
                .with_context(|| format!("failed to read config {}", config_path.display()))?;
            if plan()
                .migrate(&original, DEFAULT_CONFIG_TEMPLATE)?
                .is_none()
            {
                return parse_and_validate(&original, &format!("config {}", config_path.display()));
            }
        }

        config_file::with_lock(config_path, || Self::load_from_path_locked(config_path))
    }

    fn load_from_path_locked(config_path: &Path) -> Result<Self> {
        if !config_path.exists() {
            config_file::atomic_write(config_path, DEFAULT_CONFIG_TEMPLATE.as_bytes(), None)?;
            return parse_and_validate(DEFAULT_CONFIG_TEMPLATE, "embedded template");
        }

        let original = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read config {}", config_path.display()))?;
        let migration = plan().migrate(&original, DEFAULT_CONFIG_TEMPLATE)?;
        let content = migration
            .as_ref()
            .map_or(original.as_str(), |migration| migration.content.as_str());

        // The user's file is replaced only once the migrated document is known
        // to load and validate.
        let config = parse_and_validate(content, &format!("config {}", config_path.display()))?;

        if let Some(migration) = migration {
            config_file::apply_migration(config_path, original.as_bytes(), &migration)?;
            debug_assert_eq!(config.version, migration.to_version);
        }

        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.knowledge.chunk_overlap >= self.knowledge.chunk_size {
            anyhow::bail!(
                "Invalid knowledge configuration: chunk_overlap ({}) must be less than chunk_size ({})",
                self.knowledge.chunk_overlap,
                self.knowledge.chunk_size
            );
        }

        Ok(())
    }
}

fn parse_and_validate(content: &str, source: &str) -> Result<Config> {
    let config: Config = toml::from_str(content)
        .with_context(|| format!("Config validation failed for {source}"))?;
    config
        .validate()
        .with_context(|| format!("Config validation failed for {source}"))?;
    Ok(config)
}

/// `serde` default for configs written before the version stamp existed.
fn legacy_config_version() -> u32 {
    LEGACY_CONFIG_VERSION
}

/// Backups sitting next to `config_path`. The naming scheme is octolib's, so
/// tests here only ever ask whether a backup was made, never what it's called.
#[cfg(test)]
fn backups(config_path: &Path) -> Vec<std::path::PathBuf> {
    let parent = config_path
        .parent()
        .expect("config path must have a parent");
    std::fs::read_dir(parent)
        .expect("config directory must be readable")
        .map(|entry| entry.expect("directory entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bak"))
        .collect()
}

/// Reranker configuration for improving search result accuracy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankerConfig {
    /// Enable reranking for memory search
    pub enabled: bool,
    /// Reranker model (fully qualified, e.g., "voyage:rerank-2.5")
    pub model: String,
    /// Number of candidates to retrieve before reranking
    pub top_k_candidates: usize,
    /// Number of results to return after reranking
    pub final_top_k: usize,
    /// Timeout in seconds for reranker calls (0 = disabled)
    pub timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use octolib::utils::config_migration::{merge_missing, VersionMigration};
    use toml_edit::DocumentMut;

    fn legacy_v1_config() -> String {
        DEFAULT_CONFIG_TEMPLATE.replacen("version = 1\n\n", "", 1)
    }

    #[test]
    fn missing_config_is_exact_copy_of_embedded_template() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.toml");

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            DEFAULT_CONFIG_TEMPLATE.as_bytes()
        );
    }

    #[test]
    fn released_unversioned_config_loads_as_v1_without_rewrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = legacy_v1_config()
            .replacen(
                "similarity_threshold = 0.3",
                "# Keep this user choice\nsimilarity_threshold = 0.77",
                1,
            )
            .replacen(
                "[embedding]",
                "custom_extension = \"keep-me\"\n\n[embedding]",
                1,
            );
        std::fs::write(&path, &original).unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(config.search.similarity_threshold, 0.77);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert!(backups(&path).is_empty());
    }

    #[test]
    fn current_config_is_not_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = DEFAULT_CONFIG_TEMPLATE.replacen(
            "similarity_threshold = 0.3",
            "# current config comment\nsimilarity_threshold = 0.61",
            1,
        );
        std::fs::write(&path, &original).unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.search.similarity_threshold, 0.61);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert!(backups(&path).is_empty());
    }

    #[test]
    fn invalid_and_future_configs_are_left_untouched() {
        let directory = tempfile::tempdir().unwrap();

        let invalid_path = directory.path().join("invalid.toml");
        let invalid = legacy_v1_config().replacen("chunk_overlap = 300", "chunk_overlap = 1200", 1);
        std::fs::write(&invalid_path, &invalid).unwrap();
        assert!(Config::load_from_path(&invalid_path).is_err());
        assert_eq!(std::fs::read(&invalid_path).unwrap(), invalid.as_bytes());
        assert!(backups(&invalid_path).is_empty());

        let future_path = directory.path().join("future.toml");
        let future = DEFAULT_CONFIG_TEMPLATE.replacen("version = 1", "version = 2", 1);
        std::fs::write(&future_path, &future).unwrap();
        assert!(Config::load_from_path(&future_path).is_err());
        assert_eq!(std::fs::read(&future_path).unwrap(), future.as_bytes());
        assert!(backups(&future_path).is_empty());
    }

    /// The chain is currently empty (v1 is the only released schema); this is
    /// the contract a future `VersionMigration` will run under: steps execute
    /// in order, once each, and the driver stamps every intermediate version.
    #[test]
    fn chain_walks_v1_to_v3_one_step_at_a_time() {
        const V3_TEMPLATE: &str = "version = 3\n\n[two]\na = 1\n\n[three]\nb = 2\n";

        fn add_two(document: &mut DocumentMut, template: &DocumentMut) -> Result<()> {
            merge_missing(document.as_table_mut(), template.as_table(), "two")
        }
        fn add_three(document: &mut DocumentMut, template: &DocumentMut) -> Result<()> {
            merge_missing(document.as_table_mut(), template.as_table(), "three")
        }

        let chained = MigrationPlan::new(
            "octobrain",
            vec![
                VersionMigration {
                    from: 1,
                    to: 2,
                    apply: add_two,
                },
                VersionMigration {
                    from: 2,
                    to: 3,
                    apply: add_three,
                },
            ],
        )
        .with_missing_version(LEGACY_CONFIG_VERSION);

        // No version field at all: treated as v1 and walked all the way to v3.
        let migration = chained
            .migrate("# user comment\ncustom = \"keep\"\n", V3_TEMPLATE)
            .unwrap()
            .expect("an unversioned config must migrate");

        assert_eq!(migration.from_version, 1);
        assert_eq!(migration.to_version, 3);
        assert!(migration.content.contains("# user comment"));

        let migrated: toml::Value = toml::from_str(&migration.content).unwrap();
        assert_eq!(migrated["version"].as_integer(), Some(3));
        assert_eq!(migrated["custom"].as_str(), Some("keep"));
        assert_eq!(migrated["two"]["a"].as_integer(), Some(1));
        assert_eq!(migrated["three"]["b"].as_integer(), Some(2));

        // Re-running the now-current document is a no-op.
        assert!(chained
            .migrate(&migration.content, V3_TEMPLATE)
            .unwrap()
            .is_none());
    }

    #[test]
    fn template_is_the_target_version_and_needs_no_migration() {
        assert_eq!(plan().target_version(DEFAULT_CONFIG_TEMPLATE).unwrap(), 1);
        assert!(plan()
            .migrate(DEFAULT_CONFIG_TEMPLATE, DEFAULT_CONFIG_TEMPLATE)
            .unwrap()
            .is_none());
    }

    #[test]
    fn migration_backs_up_the_original_and_replaces_atomically() {
        const V2_TEMPLATE: &str = "version = 2\n\n[two]\na = 1\n";

        fn add_two(document: &mut DocumentMut, template: &DocumentMut) -> Result<()> {
            merge_missing(document.as_table_mut(), template.as_table(), "two")
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "version = 1\n";
        std::fs::write(&path, original).unwrap();

        let chained = MigrationPlan::new(
            "octobrain",
            vec![VersionMigration {
                from: 1,
                to: 2,
                apply: add_two,
            }],
        );
        let migration = chained.migrate(original, V2_TEMPLATE).unwrap().unwrap();
        let backup = config_file::apply_migration(&path, original.as_bytes(), &migration).unwrap();

        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let written: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["version"].as_integer(), Some(2));

        // Idempotent: re-applying the same migration must not clobber the backup.
        config_file::apply_migration(&path, original.as_bytes(), &migration).unwrap();
        assert_eq!(backups(&path), vec![backup.clone()]);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
    }
}
