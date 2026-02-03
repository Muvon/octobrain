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

//! Reranker integration module for octobrain
//!
//! This module provides integration between octobrain's memory system and octolib's
//! reranker functionality. It wraps octolib's reranker API and handles
//! conversion between MemorySearchResult and document strings needed for reranking.
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::memory::reranker_integration::RerankerIntegration;
//!
//! let config = RerankerConfig {
//!     enabled: true,
//!     model: "voyage:rerank-2.5".to_string(),
//!     top_k_candidates: 50,
//!     final_top_k: 10,
//! };
//!
//! let reranker = RerankerIntegration::new(config);
//! let reranked = reranker.rerank_memories(query, results).await?;
//! ```

use crate::config::RerankerConfig;
use crate::memory::types::MemorySearchResult;
use anyhow::Result;

/// Reranker integration wrapper
pub struct RerankerIntegration {
    pub config: RerankerConfig,
}

impl RerankerIntegration {
    pub fn new(config: RerankerConfig) -> Self {
        Self { config }
    }

    /// Rerank memory search results using octolib
    pub async fn rerank_memories(
        &self,
        query: &str,
        mut results: Vec<MemorySearchResult>,
    ) -> Result<Vec<MemorySearchResult>> {
        if !self.config.enabled || results.is_empty() {
            return Ok(results);
        }

        // Parse provider and model from config
        let (provider, model) = if let Some((p, m)) = self.config.model.split_once(':') {
            (p, m)
        } else {
            return Err(anyhow::anyhow!(
                "Invalid reranker model format: {}",
                self.config.model
            ));
        };

        // Convert memories to documents for reranking
        let documents: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    "{}\n{}\nTags: {}",
                    r.memory.title,
                    r.memory.content,
                    r.memory.metadata.tags.join(", ")
                )
            })
            .collect();

        // Call octolib reranker
        let rerank_response = octolib::reranker::rerank(
            query,
            documents,
            provider,
            model,
            Some(self.config.final_top_k),
        )
        .await?;

        // Map reranked results back to MemorySearchResult
        let mut reranked_results = Vec::new();
        for rerank_result in rerank_response.results {
            if let Some(original) = results.get_mut(rerank_result.index) {
                // Update relevance score with reranker score (convert f64 to f32)
                original.relevance_score = rerank_result.relevance_score as f32;
                reranked_results.push(original.clone());
            }
        }

        Ok(reranked_results)
    }
}
