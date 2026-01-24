// Copyright 2025 Muvon Un Limited
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
use serde::{Deserialize, Serialize};

/// Embedding configuration for memory operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub model: String,
    pub batch_size: usize,
    pub max_tokens_per_batch: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "voyage:voyage-3.5-lite".to_string(),
            batch_size: 32,
            max_tokens_per_batch: 100000,
        }
    }
}

/// Search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub similarity_threshold: f32,
    pub max_results: usize,
    /// Hybrid search configuration
    #[serde(default)]
    pub hybrid: HybridSearchConfig,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.3,
            max_results: 50,
            hybrid: HybridSearchConfig::default(),
        }
    }
}

/// Hybrid search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchConfig {
    /// Enable hybrid search (vector + keyword + recency + importance)
    pub enabled: bool,
    /// Default weight for vector similarity signal
    pub default_vector_weight: f32,
    /// Default weight for keyword matching signal
    pub default_keyword_weight: f32,
    /// Default weight for recency signal
    pub default_recency_weight: f32,
    /// Default weight for importance signal
    pub default_importance_weight: f32,
    /// Recency decay period in days
    pub recency_decay_days: u32,
    /// Weight for keyword matches in title
    pub keyword_title_weight: f32,
    /// Weight for keyword matches in content
    pub keyword_content_weight: f32,
    /// Weight for keyword matches in tags
    pub keyword_tags_weight: f32,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_vector_weight: 0.6,
            default_keyword_weight: 0.2,
            default_recency_weight: 0.1,
            default_importance_weight: 0.1,
            recency_decay_days: 30,
            keyword_title_weight: 3.0,
            keyword_content_weight: 1.0,
            keyword_tags_weight: 2.0,
        }
    }
}

/// Main configuration for octobrain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub embedding: EmbeddingConfig,
    pub search: SearchConfig,
}

impl Config {
    /// Load configuration from config.toml file
    /// First tries to load from system config directory, falls back to embedded template
    pub fn load() -> Result<Self> {
        // Try to load from system config directory
        let config_path = crate::storage::get_system_config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Self = toml::from_str(&content)?;
            Ok(config)
        } else {
            // Config doesn't exist, create from template
            let template_content = include_str!("../config-templates/default.toml");
            let config: Self = toml::from_str(template_content)?;

            // Save to system config directory
            if let Some(parent) = config_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(&config_path, template_content)?;

            Ok(config)
        }
    }
}
