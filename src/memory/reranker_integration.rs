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
//! use octobrain::memory::reranker_integration::RerankerIntegration;
//! use octobrain::config::RerankerConfig;
//! use octobrain::memory::types::MemorySearchResult;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = RerankerConfig {
//!     enabled: true,
//!     model: "voyage:rerank-2.5".to_string(),
//!     top_k_candidates: 50,
//!     final_top_k: 10,
//!     timeout_secs: 30,
//! };
//!
//! let reranker = RerankerIntegration::new(config);
//! let query = "example query";
//! let results: Vec<MemorySearchResult> = vec![];
//! let reranked = reranker.rerank_memories(query, results, 10).await?;
//! # Ok(())
//! # }
//! ```

use crate::config::RerankerConfig;
use crate::memory::types::MemorySearchResult;
use anyhow::Result;

/// Reranker integration wrapper
#[derive(Clone)]
pub struct RerankerIntegration {
    pub config: RerankerConfig,
}

impl RerankerIntegration {
    pub fn new(config: RerankerConfig) -> Self {
        Self { config }
    }

    /// Rerank memory search results using octolib.
    ///
    /// `top_n` is the number of results the CALLER asked for — it is what we
    /// request from the reranker and the size we cap the output at. Previously
    /// this used the fixed `config.final_top_k`, which silently overrode the
    /// caller's `limit` (a `limit=5` request returned up to 10; a `limit=20`
    /// request returned only 10). On any reranker failure we degrade gracefully
    /// to the already-good pre-rerank ranking (truncated to `top_n`) instead of
    /// failing the whole search.
    pub async fn rerank_memories(
        &self,
        query: &str,
        mut results: Vec<MemorySearchResult>,
        top_n: usize,
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

        // Call octolib reranker with optional timeout
        let rerank_fut = octolib::reranker::rerank(query, documents, provider, model, Some(top_n));
        let rerank_outcome = if self.config.timeout_secs == 0 {
            rerank_fut.await
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(self.config.timeout_secs),
                rerank_fut,
            )
            .await
            {
                Ok(inner) => inner,
                Err(_) => Err(anyhow::anyhow!(
                    "Reranker timed out after {}s",
                    self.config.timeout_secs
                )),
            }
        };

        // Graceful degradation: a reranker outage/timeout must not fail the whole
        // search — fall back to the pre-rerank hybrid ranking, capped to top_n.
        let rerank_response = match rerank_outcome {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("Reranker failed ({}); falling back to hybrid ranking", e);
                results.truncate(top_n);
                return Ok(results);
            }
        };

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
