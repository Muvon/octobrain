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
use serde_json::{json, Value};
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

    /// Get all tool definitions for knowledge operations
    pub fn get_tool_definitions() -> Vec<crate::mcp::types::McpTool> {
        vec![crate::mcp::types::McpTool {
            name: "knowledge_search".to_string(),
            description: "Extract specific information from a webpage URL by searching its content semantically (auto-indexes on-the-fly). PRIMARY USE CASE: When you need to find specific information within a given webpage without reading the entire page - just provide the URL and your search query. The tool will fetch, index, and search the page content automatically. SECONDARY USE CASE: If no source_url provided, searches across all previously indexed webpages in the knowledge base. This is NOT for general web search - use this when you have a specific webpage URL and want to extract relevant information from it efficiently.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What information are you looking for? Describe in natural language (e.g., 'installation instructions', 'API authentication methods', 'pricing tiers')",
                        "minLength": 3,
                        "maxLength": 500
                    },
                    "source_url": {
                        "type": "string",
                        "description": "RECOMMENDED: The specific webpage URL to search within (e.g., 'https://docs.example.com/api'). If provided, the page will be automatically fetched and indexed if not already cached. If omitted, searches across all previously indexed pages.",
                        "pattern": "^https?://"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        }]
    }

    /// Execute knowledge search
    pub async fn execute_knowledge_search(&self, arguments: &Value) -> Result<String, McpError> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter: query", "knowledge_search")
            })?;

        let source_url = arguments.get("source_url").and_then(|v| v.as_str());

        let manager = self.knowledge_manager.lock().await;
        let results = manager.search(query, source_url).await.map_err(|e| {
            McpError::internal_error(
                format!("Knowledge search failed: {}", e),
                "knowledge_search",
            )
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
            output.push_str(&result.chunk.source_url);
            output.push('\n');

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
}
