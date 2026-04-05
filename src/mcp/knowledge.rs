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
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::Config;
use crate::knowledge::KnowledgeManager;
use crate::mcp::types::McpError;

/// Knowledge tools provider
#[derive(Clone)]
pub struct KnowledgeProvider {
    knowledge_manager: Arc<Mutex<KnowledgeManager>>,
    max_results: usize,
}

impl KnowledgeProvider {
    pub async fn new(config: &Config) -> Result<Self, McpError> {
        let manager = KnowledgeManager::new(config).await.map_err(|e| {
            McpError::internal_error(
                format!("Failed to initialize knowledge manager: {}", e),
                "knowledge_init",
            )
        })?;

        Ok(Self {
            knowledge_manager: Arc::new(Mutex::new(manager)),
            max_results: config.knowledge.max_results,
        })
    }

    /// Execute search command
    pub async fn execute_search(
        &self,
        query: Option<&str>,
        source: Option<&str>,
        session_id: &str,
    ) -> Result<String, McpError> {
        let query = query.ok_or_else(|| {
            McpError::invalid_params(
                "Missing required parameter: query (required for search command)",
                "knowledge",
            )
        })?;

        let manager = self.knowledge_manager.lock().await;
        let results = manager
            .search(query, source, Some(session_id))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Knowledge search failed: {}", e), "knowledge")
            })?;

        if results.is_empty() {
            return Ok("No results found".to_string());
        }

        let mut output = String::new();
        for result in results.iter().take(self.max_results) {
            output.push_str(&"=".repeat(50));
            output.push('\n');
            output.push_str(&result.chunk.source_title);
            output.push('\n');
            output.push_str(&result.chunk.source);
            output.push('\n');

            if result.session_scoped {
                output.push_str("[SESSION] ");
            }

            if !result.chunk.section_path.is_empty() {
                output.push_str(&result.chunk.section_path.join(" > "));
                output.push('\n');
            }

            // Show content preview (first 300 chars)
            let content_preview = if result.chunk.content.chars().count() > 300 {
                format!(
                    "{}...",
                    result.chunk.content.chars().take(300).collect::<String>()
                )
            } else {
                result.chunk.content.clone()
            };
            output.push_str(&content_preview);
            output.push('\n');

            let score_pct = (result.relevance_score * 100.0) as u32;
            output.push_str(&format!("Relevance: {}%\n\n", score_pct));
        }

        Ok(output)
    }

    /// Execute store command
    pub async fn execute_store(
        &self,
        key: Option<&str>,
        content: Option<&str>,
        session_id: &str,
    ) -> Result<String, McpError> {
        let key = key.ok_or_else(|| {
            McpError::invalid_params(
                "Missing required parameter: key (required for store command)",
                "knowledge",
            )
        })?;
        let content = content.ok_or_else(|| {
            McpError::invalid_params(
                "Missing required parameter: content (required for store command)",
                "knowledge",
            )
        })?;

        let manager = self.knowledge_manager.lock().await;
        let result = manager
            .store_content(key, content, session_id)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Knowledge store failed: {}", e), "knowledge")
            })?;

        Ok(format!(
            "Stored '{}' as {} ({} chunks indexed)",
            key, result.source, result.chunks_created
        ))
    }

    /// Execute delete command
    pub async fn execute_delete(
        &self,
        key: Option<&str>,
        session_id: &str,
    ) -> Result<String, McpError> {
        let key = key.ok_or_else(|| {
            McpError::invalid_params(
                "Missing required parameter: key (required for delete command)",
                "knowledge",
            )
        })?;

        let manager = self.knowledge_manager.lock().await;
        manager.delete_content(key, session_id).await.map_err(|e| {
            McpError::internal_error(format!("Knowledge delete failed: {}", e), "knowledge")
        })?;

        Ok(format!("Deleted stored knowledge '{}'", key))
    }
}
