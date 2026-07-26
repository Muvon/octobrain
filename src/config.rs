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

use std::fs::Permissions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use toml_edit::DocumentMut;

use crate::memory::types::MemoryConfig;

const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../config-templates/default.toml");
const LEGACY_CONFIG_VERSION: u32 = 1;

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
        Self::load_from_path_with(config_path, persist_migrated_config)
    }

    fn load_from_path_with<P>(config_path: &Path, persist_migration: P) -> Result<Self>
    where
        P: FnOnce(&Path, &[u8], &[u8]) -> Result<()>,
    {
        let template_version = required_version(DEFAULT_CONFIG_TEMPLATE, "embedded template")?;
        let template_config = parse_and_validate(DEFAULT_CONFIG_TEMPLATE, "embedded template")?;

        if !config_path.exists() {
            create_config_exactly(config_path, DEFAULT_CONFIG_TEMPLATE.as_bytes())?;
            return Ok(template_config);
        }

        let original = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read config {}", config_path.display()))?;
        let mut document = original
            .parse::<DocumentMut>()
            .with_context(|| format!("config {} is not valid TOML", config_path.display()))?;
        let user_version = user_version(&document)?;

        if user_version > template_version {
            anyhow::bail!(
                "Config {} has version {}, but this binary only supports version {}. Upgrade octobrain before using this config.",
                config_path.display(),
                user_version,
                template_version
            );
        }

        if user_version == template_version {
            return parse_and_validate(&original, &format!("config {}", config_path.display()));
        }

        migrate(&mut document, user_version, template_version)?;
        let migrated = document.to_string();
        let config = parse_and_validate(
            &migrated,
            &format!("migrated config {}", config_path.display()),
        )?;

        persist_migration(config_path, original.as_bytes(), migrated.as_bytes())?;
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

fn required_version(content: &str, source: &str) -> Result<u32> {
    let document = content
        .parse::<DocumentMut>()
        .with_context(|| format!("{source} is not valid TOML"))?;
    document_version(&document)?
        .ok_or_else(|| anyhow::anyhow!("{source} does not declare a config version"))
}

fn user_version(document: &DocumentMut) -> Result<u32> {
    Ok(document_version(document)?.unwrap_or(LEGACY_CONFIG_VERSION))
}

fn legacy_config_version() -> u32 {
    LEGACY_CONFIG_VERSION
}

fn document_version(document: &DocumentMut) -> Result<Option<u32>> {
    let Some(item) = document.as_table().get("version") else {
        return Ok(None);
    };
    let version = item
        .as_integer()
        .ok_or_else(|| anyhow::anyhow!("config version must be a positive integer"))?;
    let version = u32::try_from(version)
        .map_err(|_| anyhow::anyhow!("config version must be a positive integer"))?;
    if version == 0 {
        anyhow::bail!("config version must be a positive integer");
    }
    Ok(Some(version))
}

fn migrate(document: &mut DocumentMut, version: u32, target_version: u32) -> Result<()> {
    migrate_with(document, version, target_version, migrate_one_version)
}

fn migrate_with<M>(
    document: &mut DocumentMut,
    mut version: u32,
    target_version: u32,
    mut migrate_one: M,
) -> Result<()>
where
    M: FnMut(&mut DocumentMut, u32) -> Result<()>,
{
    while version < target_version {
        migrate_one(document, version)?;

        let migrated_version = document_version(document)?.ok_or_else(|| {
            anyhow::anyhow!("config migration from version {version} did not set a version")
        })?;
        if migrated_version != version + 1 {
            anyhow::bail!(
                "config migration from version {} produced version {}, expected {}",
                version,
                migrated_version,
                version + 1
            );
        }
        version = migrated_version;
    }

    Ok(())
}

fn migrate_one_version(_document: &mut DocumentMut, version: u32) -> Result<()> {
    // Add an explicit arm here only when the matching schema version is released.
    anyhow::bail!(
        "No config migration exists from version {} to version {}",
        version,
        version + 1
    )
}

#[allow(dead_code)] // Used by explicit migration functions once version 2 exists.
fn set_version(document: &mut DocumentMut, version: u32) -> Result<()> {
    if version == 0 {
        anyhow::bail!("config version must be a positive integer");
    }

    let next_version = toml_edit::Value::from(i64::from(version));
    if let Some(version_item) = document.as_table_mut().get_mut("version") {
        let decor = version_item
            .as_value()
            .ok_or_else(|| anyhow::anyhow!("config version must be a positive integer"))?
            .decor()
            .clone();
        let mut next_version = next_version;
        *next_version.decor_mut() = decor;
        *version_item = toml_edit::Item::Value(next_version);
    } else {
        document
            .as_table_mut()
            .insert("version", toml_edit::Item::Value(next_version));
    }

    Ok(())
}

fn create_config_exactly(config_path: &Path, content: &[u8]) -> Result<()> {
    let parent = parent_directory(config_path);
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory {}", parent.display()))?;

    let temp = prepare_temp_file(config_path, content, None)?;
    temp.persist_noclobber(config_path).map_err(|error| {
        anyhow::anyhow!(
            "failed to create config {}: {}",
            config_path.display(),
            error.error
        )
    })?;
    Ok(())
}

fn persist_migrated_config(config_path: &Path, original: &[u8], migrated: &[u8]) -> Result<()> {
    persist_migrated_config_with(config_path, original, migrated, |temp, path| {
        temp.persist(path).map_err(|error| {
            anyhow::anyhow!(
                "failed to atomically replace config {}: {}",
                path.display(),
                error.error
            )
        })?;
        Ok(())
    })
}

fn persist_migrated_config_with<R>(
    config_path: &Path,
    original: &[u8],
    migrated: &[u8],
    replace: R,
) -> Result<()>
where
    R: FnOnce(NamedTempFile, &Path) -> Result<()>,
{
    let permissions = std::fs::metadata(config_path)
        .with_context(|| format!("failed to inspect config {}", config_path.display()))?
        .permissions();
    let replacement = prepare_temp_file(config_path, migrated, Some(permissions.clone()))?;

    let backup_path = backup_path(config_path);
    let backup = prepare_temp_file(&backup_path, original, Some(permissions))?;
    backup.persist(&backup_path).map_err(|error| {
        anyhow::anyhow!(
            "failed to write config backup {}: {}",
            backup_path.display(),
            error.error
        )
    })?;

    replace(replacement, config_path)
}

fn prepare_temp_file(
    destination: &Path,
    content: &[u8],
    permissions: Option<Permissions>,
) -> Result<NamedTempFile> {
    let parent = parent_directory(destination);
    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    temp.write_all(content)
        .with_context(|| format!("failed to write temporary config in {}", parent.display()))?;
    if let Some(permissions) = permissions {
        temp.as_file()
            .set_permissions(permissions)
            .context("failed to preserve config permissions")?;
    }
    temp.as_file()
        .sync_all()
        .context("failed to sync temporary config")?;
    Ok(temp)
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn backup_path(config_path: &Path) -> PathBuf {
    let mut path = config_path.as_os_str().to_os_string();
    path.push(".backup");
    PathBuf::from(path)
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
        assert!(!backup_path(&path).exists());
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
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn migration_engine_runs_each_version_once_and_is_idempotent() {
        let mut document = "# user comment\nversion = 1\ncustom = \"keep\"\n"
            .parse::<DocumentMut>()
            .unwrap();
        let mut steps = Vec::new();

        migrate_with(&mut document, 1, 3, |document, version| {
            steps.push(version);
            set_version(document, version + 1)
        })
        .unwrap();

        assert_eq!(steps, vec![1, 2]);
        assert_eq!(document_version(&document).unwrap(), Some(3));
        assert!(document.to_string().contains("# user comment"));
        assert!(document.to_string().contains("custom = \"keep\""));

        let migrated = document.to_string();
        migrate_with(&mut document, 3, 3, |_document, _version| {
            panic!("an at-target migration must not run")
        })
        .unwrap();
        assert_eq!(document.to_string(), migrated);

        let mut unversioned = "# legacy comment\ncustom = \"keep\"\n"
            .parse::<DocumentMut>()
            .unwrap();
        set_version(&mut unversioned, 2).unwrap();
        assert_eq!(document_version(&unversioned).unwrap(), Some(2));
        assert!(unversioned.to_string().contains("# legacy comment"));
        assert!(unversioned.to_string().contains("custom = \"keep\""));
    }

    #[test]
    fn invalid_and_future_configs_are_left_untouched() {
        let directory = tempfile::tempdir().unwrap();

        let invalid_path = directory.path().join("invalid.toml");
        let invalid = legacy_v1_config().replacen("chunk_overlap = 300", "chunk_overlap = 1200", 1);
        std::fs::write(&invalid_path, &invalid).unwrap();
        assert!(Config::load_from_path(&invalid_path).is_err());
        assert_eq!(std::fs::read(&invalid_path).unwrap(), invalid.as_bytes());
        assert!(!backup_path(&invalid_path).exists());

        let future_path = directory.path().join("future.toml");
        let future = DEFAULT_CONFIG_TEMPLATE.replacen("version = 1", "version = 2", 1);
        std::fs::write(&future_path, &future).unwrap();
        assert!(Config::load_from_path(&future_path).is_err());
        assert_eq!(std::fs::read(&future_path).unwrap(), future.as_bytes());
        assert!(!backup_path(&future_path).exists());
    }

    #[test]
    fn failed_atomic_replace_leaves_original_config_intact() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = DEFAULT_CONFIG_TEMPLATE.to_owned();
        let replacement = original.replacen("version = 1", "version = 2", 1);
        std::fs::write(&path, &original).unwrap();

        let result = persist_migrated_config_with(
            &path,
            original.as_bytes(),
            replacement.as_bytes(),
            |_temp, _path| anyhow::bail!("simulated atomic replace failure"),
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(
            std::fs::read(backup_path(&path)).unwrap(),
            original.as_bytes()
        );
    }

    #[test]
    fn successful_persistence_backs_up_and_atomically_replaces_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = DEFAULT_CONFIG_TEMPLATE.to_owned();
        let replacement = original.replacen("version = 1", "version = 2", 1);
        std::fs::write(&path, &original).unwrap();

        persist_migrated_config(&path, original.as_bytes(), replacement.as_bytes()).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), replacement.as_bytes());
        assert_eq!(
            std::fs::read(backup_path(&path)).unwrap(),
            original.as_bytes()
        );
    }
}
