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

use tracing::{debug, warn};

use crate::config::Config;
use crate::constants::MAX_QUERIES;
use crate::embedding::truncate_output;
use crate::mcp::types::McpError;
use crate::memory::{MemoryManager, MemoryQuery, MemoryType};

/// Memory tools provider
#[derive(Clone)]
pub struct MemoryProvider {
    memory_manager: Arc<Mutex<MemoryManager>>,
    working_directory: std::path::PathBuf,
}

impl MemoryProvider {
    pub async fn new(
        config: &Config,
        working_directory: std::path::PathBuf,
        project_key: Option<String>,
        role: Option<String>,
    ) -> Result<Self, McpError> {
        let original_dir = std::env::current_dir().ok();
        if let Err(e) = std::env::set_current_dir(&working_directory) {
            warn!(
                error = %e,
                "Failed to change to working directory for memory initialization"
            );
        }

        let manager = MemoryManager::new(config, project_key.clone(), role.clone())
            .await
            .map_err(|e| {
                McpError::internal_error(
                    format!("Failed to initialize memory manager: {}", e),
                    "memory_init",
                )
            })?;

        if let Some(original) = original_dir {
            let _ = std::env::set_current_dir(&original);
        }

        Ok(Self {
            memory_manager: Arc::new(Mutex::new(manager)),
            working_directory,
        })
    }

    /// Get all tool definitions for memory operations.
    /// When `locked` is true, project/role are fixed server-side and hidden from tool schemas.
    /// When `locked` is false, project/role are exposed as optional tool parameters.
    pub fn get_tool_definitions(locked: bool) -> Vec<crate::mcp::types::McpTool> {
        // Extra properties injected into write tools (memorize, forget, relate) when not locked
        let scope_props_write = if locked {
            json!({})
        } else {
            json!({
                "project": {
                    "type": "string",
                    "description": "Project key to scope this memory to. Defaults to auto-detected Git remote hash."
                },
                "role": {
                    "type": "string",
                    "description": "Role tag to attach to this memory (e.g. 'developer', 'reviewer')."
                }
            })
        };
        // Extra properties injected into read tools (remember, auto_link, memory_graph) when not locked
        let scope_props_read = if locked {
            json!({})
        } else {
            json!({
                "project": {
                    "type": "string",
                    "description": "Filter by project key. If omitted, returns memories from all projects."
                },
                "role": {
                    "type": "string",
                    "description": "Filter by role. If omitted, returns memories for all roles."
                }
            })
        };
        vec![
			crate::mcp::types::McpTool {
				name: "memorize".to_string(),
				description: "Store information, insights, or context in memory. Call remember first to avoid duplicates. Set source='user_confirmed' for user-stated facts (importance 0.8-1.0), 'agent_inferred' for AI conclusions (0.3-0.6). Skip transient state or things easily re-derived.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"title": {
							"type": "string",
							"description": "Short descriptive title",
						},
						"content": {
							"type": "string",
							"description": "Full content — explanations, code snippets, decisions, etc.",
						},
						"memory_type": {
							"type": "string",
							"description": "Memory category",
							"enum": ["code", "architecture", "bug_fix", "feature", "documentation", "user_preference", "decision", "learning", "configuration", "testing", "performance", "security", "insight"],
							"default": "code"
						},
						"importance": {
							"type": "number",
							"description": "Importance 0.0-1.0: user facts 0.8-1.0, decisions 0.7-0.9, bug fixes 0.6-0.8, inferences 0.3-0.6",
							"minimum": 0.0,
							"maximum": 1.0,
							"default": 0.5
						},
						"tags": {
							"type": "array",
							"description": "Tags for categorization and filtering",
							"items": {
								"type": "string"
							},
							"maxItems": 10
						},
						"related_files": {
							"type": "array",
							"description": "File paths related to this memory",
							"items": {
								"type": "string"
							},
							"maxItems": 20
						},
						"source": {
							"type": "string",
							"description": "Trust tier: 'user_confirmed' (user explicitly stated/approved) ranks higher in retrieval; 'agent_inferred' for AI conclusions",
							"enum": ["user_confirmed", "agent_inferred"],
							"default": "agent_inferred"
						}
					},
					"required": ["title", "content"],
					"additionalProperties": false
				}),
			},
			crate::mcp::types::McpTool {
				name: "remember".to_string(),
					description: "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. Prefer 2-5 related query terms for broader coverage. Results show [CONFIRMED]/[INFERRED] trust labels.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"query": {
							"oneOf": [
								{
									"type": "string",
									"description": "Single semantic search query",
									"minLength": 3,
									"maxLength": 500
								},
								{
									"type": "array",
									"items": {
										"type": "string",
										"minLength": 3,
										"maxLength": 500
									},
									"minItems": 1,
									"maxItems": 5,
									"description": "2-5 related terms for comprehensive coverage — preferred over single query"
								}
							],
							"description": "String or array of 2-5 related terms. Array preferred for broader semantic coverage."
						},
						"memory_types": {
							"type": "array",
							"description": "Narrow results to specific memory categories",
							"items": {
								"type": "string",
								"enum": ["code", "architecture", "bug_fix", "feature", "documentation", "user_preference", "decision", "learning", "configuration", "testing", "performance", "security", "insight"]
							}
						},
						"tags": {
							"type": "array",
							"description": "Filter by tags",
							"items": {
								"type": "string"
							}
						},
						"related_files": {
							"type": "array",
							"description": "Filter by related file paths",
							"items": {
								"type": "string"
							}
						},
						"limit": {
							"type": "integer",
							"description": "Max memories to return",
							"minimum": 1,
							"maximum": 5,
							"default": 5
						},
						"max_tokens": {
							"type": "integer",
							"description": "Output token limit (0 = unlimited)",
							"minimum": 0,
							"default": 2000
						}
					},
					"required": ["query"],
					"additionalProperties": false
				}),
			},
			crate::mcp::types::McpTool {
				name: "forget".to_string(),
					description: "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal. Don't forget memories just because they're old — importance decay handles that. Only delete when information is wrong or superseded.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"memory_id": {
							"type": "string",
							"description": "ID of memory to delete (from remember results)"
						},
						"query": {
							"type": "string",
							"description": "Semantic query to find memories to delete (alternative to memory_id)"
						},
						"memory_types": {
							"type": "array",
							"description": "Filter by memory types when using query",
							"items": {
								"type": "string",
								"enum": ["code", "architecture", "bug_fix", "feature", "documentation", "user_preference", "decision", "learning", "configuration", "testing", "performance", "security", "insight"]
							}
						},
						"tags": {
							"type": "array",
							"description": "Filter by tags when using query",
							"items": {
								"type": "string"
							}
						},
						"confirm": {
							"type": "boolean",
							"description": "Must be true — deletion is permanent",
							"const": true
						}
					},
					"required": ["confirm"],
					"additionalProperties": false
				}),
			},
			crate::mcp::types::McpTool {
				name: "auto_link".to_string(),
					description: "Find and connect semantically similar memories for a given memory ID. Auto-linking runs on new memories automatically — call this manually to refresh links after content updates or for memories created outside normal flow.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"memory_id": {
							"type": "string",
							"description": "Memory ID to auto-link"
						}
					},
					"required": ["memory_id"],
					"additionalProperties": false
				}),
			},
			crate::mcp::types::McpTool {
				name: "memory_graph".to_string(),
					description: "Retrieve a memory and its connected neighbors as a graph. remember already includes 1-hop neighbors — use this only for deeper traversal (depth > 1) or to see the full relationship structure.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"memory_id": {
							"type": "string",
							"description": "Root memory ID"
						},
						"depth": {
							"type": "integer",
							"description": "Traversal depth (default 2; use 3+ for broad exploration)",
							"minimum": 1,
							"maximum": 5,
							"default": 2
						},
						"max_tokens": {
							"type": "integer",
							"description": "Output token limit (0 = unlimited)",
							"minimum": 0,
							"default": 2000
						}
					},
					"required": ["memory_id"],
					"additionalProperties": false
				}),
			},
			crate::mcp::types::McpTool {
				name: "relate".to_string(),
					description: "Create a typed relationship between two memories. Use when auto-linking missed a meaningful connection or you need a specific type. Types: related_to, depends_on, supersedes, similar, conflicts, implements, extends. Strength 0.9+ = strong, 0.5-0.8 = moderate, <0.5 = weak.".to_string(),
				input_schema: json!({
					"type": "object",
					"properties": {
						"source_id": {
							"type": "string",
							"description": "Source memory ID"
						},
						"target_id": {
							"type": "string",
							"description": "Target memory ID"
						},
						"relationship_type": {
							"type": "string",
							"description": "Relationship type: related_to (general), depends_on (A needs B), supersedes (A replaces B), similar (near-duplicate), conflicts (contradicts), implements (concrete of abstract), extends (builds on)",
							"enum": ["related_to", "depends_on", "supersedes", "similar", "conflicts", "implements", "extends"]
						},
						"strength": {
							"type": "number",
							"description": "Relationship strength 0.0-1.0",
							"minimum": 0.0,
							"maximum": 1.0,
							"default": 0.8
						},
						"description": {
							"type": "string",
							"description": "Why these memories are related",
							"maxLength": 500
						}
					},
					"required": ["source_id", "target_id", "relationship_type"],
					"additionalProperties": false
				}),
			}
		]
        .into_iter()
        .map(|mut tool| {
            // Merge scope props into each tool's input_schema.properties
            let extra = if matches!(tool.name.as_str(), "memorize" | "forget" | "relate") {
                &scope_props_write
            } else {
                &scope_props_read
            };
            if let Some(obj) = extra.as_object() {
                if !obj.is_empty() {
                    if let Some(props) = tool.input_schema
                        .get_mut("properties")
                        .and_then(|p| p.as_object_mut())
                    {
                        for (k, v) in obj {
                            props.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            tool
        })
        .collect()
    }

    /// Execute the memorize tool with enhanced error handling
    pub async fn execute_memorize(&self, arguments: &Value) -> Result<String, McpError> {
        // Validate input parameters exist before processing
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'title'", "memorize")
            })?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'content'", "memorize")
            })?;

        // Ensure clean UTF-8 content using lossy conversion
        let clean_title = String::from_utf8_lossy(title.as_bytes()).to_string();
        let clean_content = String::from_utf8_lossy(content.as_bytes()).to_string();
        let title = clean_title.as_str();
        let content = clean_content.as_str();

        // Validate lengths directly on original content
        if title.len() < 5 || title.len() > 200 {
            return Err(McpError::invalid_params(
                "Title must be between 5 and 200 characters",
                "memorize",
            ));
        }
        if content.len() < 10 || content.len() > 10000 {
            return Err(McpError::invalid_params(
                "Content must be between 10 and 10000 characters",
                "memorize",
            ));
        }

        let memory_type_str = arguments
            .get("memory_type")
            .and_then(|v| v.as_str())
            .unwrap_or("code");

        let memory_type = MemoryType::from(memory_type_str.to_string());

        let importance = arguments
            .get("importance")
            .and_then(|v| v.as_f64())
            .map(|v| {
                // Clamp importance to valid range
                (v as f32).clamp(0.0, 1.0)
            });

        // Process tags with error handling and UTF-8 safety
        let tags = arguments.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str().and_then(|s| {
                        // Ensure clean UTF-8 and validate tag
                        let clean_tag = String::from_utf8_lossy(s.as_bytes()).to_string();
                        let tag = clean_tag.trim();

                        if tag.is_empty() {
                            None // Skip empty tags
                        } else {
                            // Limit tag length
                            let final_tag = if tag.chars().count() > 50 {
                                tag.chars().take(50).collect()
                            } else {
                                tag.to_string()
                            };
                            Some(final_tag)
                        }
                    })
                })
                .take(10) // Limit number of tags
                .collect::<Vec<String>>()
        });

        let related_files = arguments
            .get("related_files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_str().and_then(|s| {
                            // Ensure clean UTF-8 and validate file path
                            let clean_path = String::from_utf8_lossy(s.as_bytes()).to_string();
                            let path = clean_path.trim();

                            if path.is_empty() || path.len() > 500 {
                                None // Skip empty or overly long paths
                            } else {
                                Some(path.to_string())
                            }
                        })
                    })
                    .take(20) // Limit number of files
                    .collect::<Vec<String>>()
            });

        let source = arguments
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| crate::memory::types::MemorySource::from(s.to_string()));

        // Use structured logging instead of console output for MCP protocol compliance
        debug!(
            title = %title,
            memory_type = ?memory_type,
            importance = ?importance,
            "Memorizing new content"
        );

        // Change to working directory for Git context with error handling
        let original_dir = std::env::current_dir().map_err(|e| {
            McpError::internal_error(
                format!("Failed to get current directory: {}", e),
                "memorize",
            )
        })?;

        if let Err(e) = std::env::set_current_dir(&self.working_directory) {
            return Err(McpError::internal_error(
                format!("Failed to change to working directory: {}", e),
                "memorize",
            )
            .with_details(format!("Path: {}", self.working_directory.display())));
        }

        let memory_result = {
            // Lock memory manager for storing - removed timeout to allow embedding generation to complete
            let mut manager_guard = self.memory_manager.lock().await;

            manager_guard
                .memorize(crate::memory::manager::MemorizeParams {
                    memory_type,
                    title: title.to_string(),
                    content: content.to_string(),
                    importance,
                    tags,
                    related_files,
                    source,
                })
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("Failed to store memory: {}", e), "memorize")
                })?
        };

        // Restore original directory regardless of result
        if let Err(e) = std::env::set_current_dir(&original_dir) {
            warn!(
                error = %e,
                "Failed to restore original directory"
            );
        }

        let memory = memory_result;

        // Return plain text response for MCP protocol compliance
        // Return minimal response for MCP protocol compliance - just success and ID
        Ok(format!("Memory stored: {}", memory.id))
    }

    /// Execute the remember tool
    pub async fn execute_remember(&self, arguments: &Value) -> Result<String, McpError> {
        // Parse queries - handle both string and array inputs
        let queries: Vec<String> = match arguments.get("query") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => {
                let queries: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();

                if queries.is_empty() {
                    return Err(McpError::invalid_params(
                        "Invalid query array: must contain at least one non-empty string",
                        "remember",
                    ));
                }

                queries
            }
            _ => {
                return Err(McpError::invalid_params(
					"Missing required parameter 'query': must be a string or array of strings describing what to search for",
					"remember"
				));
            }
        };

        // Validate queries
        if queries.len() > MAX_QUERIES {
            return Err(McpError::invalid_params(
				format!("Too many queries: maximum {} queries allowed, got {}. Use fewer, more specific terms.", MAX_QUERIES, queries.len()),
				"remember"
			));
        }

        for (i, query) in queries.iter().enumerate() {
            // Ensure clean UTF-8 and validate query
            let clean_query = String::from_utf8_lossy(query.as_bytes()).to_string();
            let query = clean_query.trim();

            if query.len() < 3 {
                return Err(McpError::invalid_params(
                    format!(
                        "Invalid query {}: must be at least 3 characters long",
                        i + 1
                    ),
                    "remember",
                ));
            }
            if query.len() > 500 {
                return Err(McpError::invalid_params(
                    format!(
                        "Invalid query {}: must be no more than 500 characters long",
                        i + 1
                    ),
                    "remember",
                ));
            }
            if query.is_empty() {
                return Err(McpError::invalid_params(
                    format!(
                        "Invalid query {}: cannot be empty or whitespace only",
                        i + 1
                    ),
                    "remember",
                ));
            }
        }

        // Parse memory types filter
        let memory_types =
            if let Some(types_array) = arguments.get("memory_types").and_then(|v| v.as_array()) {
                let types: Vec<MemoryType> = types_array
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| MemoryType::from(s.to_string()))
                    .collect();
                if types.is_empty() {
                    None
                } else {
                    Some(types)
                }
            } else {
                None
            };

        // Parse tags filter
        let tags = if let Some(tags_array) = arguments.get("tags").and_then(|v| v.as_array()) {
            let tag_list: Vec<String> = tags_array
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if tag_list.is_empty() {
                None
            } else {
                Some(tag_list)
            }
        } else {
            None
        };

        // Parse related files filter
        let related_files =
            if let Some(files_array) = arguments.get("related_files").and_then(|v| v.as_array()) {
                let file_list: Vec<String> = files_array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if file_list.is_empty() {
                    None
                } else {
                    Some(file_list)
                }
            } else {
                None
            };

        // Set limit
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(5);

        // Parse max_tokens parameter
        let max_tokens = arguments
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000) as usize;

        let memory_query = MemoryQuery {
            memory_types,
            tags,
            related_files,
            limit: Some(limit.min(50)),
            ..Default::default()
        };

        // Use structured logging instead of console output for MCP protocol compliance
        debug!(
            queries = ?queries,
            limit = memory_query.limit.unwrap_or(10),
            "Remembering memories with {} queries",
            queries.len()
        );

        let results = {
            // Lock memory manager for searching - removed timeout to allow operations to complete
            let manager_guard = self.memory_manager.lock().await;

            // Use multi-query method for comprehensive search
            if queries.len() == 1 {
                manager_guard
                    .remember(&queries[0], Some(memory_query))
                    .await
                    .map_err(|e| {
                        McpError::internal_error(
                            format!("Failed to search memories: {}", e),
                            "remember",
                        )
                    })?
            } else {
                manager_guard
                    .remember_multi(&queries, Some(memory_query))
                    .await
                    .map_err(|e| {
                        McpError::internal_error(
                            format!("Failed to search memories: {}", e),
                            "remember",
                        )
                    })?
            }
        };

        if results.is_empty() {
            return Ok("No stored memories match your query. Try using different search terms, removing filters, or checking if any memories have been stored yet.".to_string());
        }

        // Collect IDs already in results so we don't duplicate them
        let result_ids: std::collections::HashSet<String> =
            results.iter().map(|r| r.memory.id.clone()).collect();

        // Fetch 1-hop graph neighbors (cap at 3 total across all results)
        let graph_neighbors: Vec<(
            crate::memory::types::Memory,
            crate::memory::types::RelationshipType,
            f32,
        )> = {
            let manager_guard = self.memory_manager.lock().await;
            let mut neighbors = Vec::new();
            'outer: for result in &results {
                let rels = manager_guard
                    .get_relationships(&result.memory.id)
                    .await
                    .unwrap_or_default();
                for rel in rels {
                    let neighbor_id = if rel.source_id == result.memory.id {
                        rel.target_id.clone()
                    } else {
                        rel.source_id.clone()
                    };
                    if result_ids.contains(&neighbor_id) {
                        continue;
                    }
                    if neighbors
                        .iter()
                        .any(|(m, _, _): &(crate::memory::types::Memory, _, _)| m.id == neighbor_id)
                    {
                        continue;
                    }
                    if let Ok(Some(mem)) = manager_guard.get_memory(&neighbor_id).await {
                        neighbors.push((mem, rel.relationship_type.clone(), rel.strength));
                        if neighbors.len() >= 3 {
                            break 'outer;
                        }
                    }
                }
            }
            neighbors
        };

        // Format primary results
        let mut output = crate::memory::format_memories_as_text(&results);

        // Append graph neighbors section if any were found
        if !graph_neighbors.is_empty() {
            output.push_str("\n--- Related context (via graph) ---\n");
            for (mem, rel_type, strength) in &graph_neighbors {
                output.push_str(&format!(
                    "\n[{}] {} (ID: {}, rel: {}, strength: {:.2})\n{}\n",
                    mem.metadata.source.display_label(),
                    mem.title,
                    mem.id,
                    rel_type,
                    strength,
                    mem.content
                ));
            }
        }

        // Apply token truncation if needed
        Ok(truncate_output(&output, max_tokens))
    }

    /// Execute the forget tool
    pub async fn execute_forget(&self, arguments: &Value) -> Result<String, McpError> {
        // Check confirm parameter
        if !arguments
            .get("confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(
                "❌ Missing required confirmation: set 'confirm' to true to proceed with deletion"
                    .to_string(),
            );
        }

        // Handle specific memory ID deletion
        if let Some(memory_id) = arguments.get("memory_id").and_then(|v| v.as_str()) {
            // Validate memory ID format
            if memory_id.trim().is_empty() || memory_id.len() > 100 {
                return Ok("❌ Invalid memory ID format".to_string());
            }

            // Use structured logging instead of console output for MCP protocol compliance
            debug!(
                memory_id = %memory_id,
                "Forgetting memory by ID"
            );

            // Execute deletion - removed timeout to allow operation to complete
            let res = {
                let mut manager_guard = self.memory_manager.lock().await;
                manager_guard.forget(memory_id).await
            };
            match res {
                Ok(_) => Ok(format!(
                    "✅ Memory deleted successfully\n\nMemory ID: {}",
                    memory_id
                )),
                Err(e) => {
                    tracing::warn!("Memory deletion failed: {}", e);
                    Ok(format!("❌ Failed to delete memory: {}", e))
                }
            }
        }
        // Handle query-based deletion
        else if let Some(query) = arguments.get("query").and_then(|v| v.as_str()) {
            // Ensure clean UTF-8 query using lossy conversion
            let clean_query = String::from_utf8_lossy(query.as_bytes()).to_string();
            let query = clean_query.as_str();

            if query.len() < 3 || query.len() > 500 {
                return Ok("❌ Query must be between 3 and 500 characters".to_string());
            }
            // Parse memory types filter
            let memory_types = if let Some(types_array) =
                arguments.get("memory_types").and_then(|v| v.as_array())
            {
                let types: Vec<MemoryType> = types_array
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| MemoryType::from(s.to_string()))
                    .collect();
                if types.is_empty() {
                    None
                } else {
                    Some(types)
                }
            } else {
                None
            };

            // Parse tags filter
            let tags = if let Some(tags_array) = arguments.get("tags").and_then(|v| v.as_array()) {
                let tag_list: Vec<String> = tags_array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if tag_list.is_empty() {
                    None
                } else {
                    Some(tag_list)
                }
            } else {
                None
            };

            let memory_query = MemoryQuery {
                query_text: Some(query.to_string()),
                memory_types,
                tags,
                ..Default::default()
            };

            // Use structured logging instead of console output for MCP protocol compliance
            debug!(
                query = %query,
                "Forgetting memories matching query"
            );

            let res = {
                let mut manager_guard = self.memory_manager.lock().await;
                manager_guard.forget_matching(memory_query).await
            };
            match res {
                Ok(deleted_count) => Ok(format!(
                    "✅ {} memories deleted successfully\n\nQuery: \"{}\"",
                    deleted_count, query
                )),
                Err(e) => Ok(format!("❌ Failed to delete memories: {}", e)),
            }
        } else {
            Ok("❌ Either 'memory_id' or 'query' must be provided".to_string())
        }
    }

    /// Execute the auto_link tool
    pub async fn execute_auto_link(&self, arguments: &Value) -> Result<String, McpError> {
        let memory_id = arguments
            .get("memory_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'memory_id'", "auto_link")
            })?;

        // Validate memory ID
        if memory_id.trim().is_empty() || memory_id.len() > 100 {
            return Err(McpError::invalid_params(
                "Invalid memory ID format",
                "auto_link",
            ));
        }

        debug!(
            memory_id = %memory_id,
            "Auto-linking memory"
        );

        let relationships = {
            let mut manager_guard = self.memory_manager.lock().await;
            manager_guard
                .auto_link_memory(memory_id)
                .await
                .map_err(|e| {
                    McpError::internal_error(
                        format!("Failed to auto-link memory: {}", e),
                        "auto_link",
                    )
                })?
        };

        if relationships.is_empty() {
            Ok(format!(
                "No similar memories found to link with '{}' (similarity threshold not met)",
                memory_id
            ))
        } else {
            let mut output = format!(
                "✅ Created {} auto-link(s) for memory '{}':\n\n",
                relationships.len(),
                memory_id
            );

            for rel in relationships.iter().take(10) {
                // Limit to 10 for readability
                output.push_str(&format!(
                    "  {} -> {} (strength: {:.2})\n",
                    rel.source_id, rel.target_id, rel.strength
                ));
            }

            if relationships.len() > 10 {
                output.push_str(&format!("\n  ... and {} more\n", relationships.len() - 10));
            }

            Ok(output)
        }
    }

    /// Execute the memory_graph tool
    pub async fn execute_memory_graph(&self, arguments: &Value) -> Result<String, McpError> {
        let memory_id = arguments
            .get("memory_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'memory_id'", "memory_graph")
            })?;

        // Validate memory ID
        if memory_id.trim().is_empty() || memory_id.len() > 100 {
            return Err(McpError::invalid_params(
                "Invalid memory ID format",
                "memory_graph",
            ));
        }

        let depth = arguments
            .get("depth")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(2)
            .clamp(1, 5);

        let max_tokens = arguments
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000) as usize;

        debug!(
            memory_id = %memory_id,
            depth = depth,
            "Building memory graph"
        );

        let graph = {
            let manager_guard = self.memory_manager.lock().await;
            manager_guard
                .get_memory_graph(memory_id, depth)
                .await
                .map_err(|e| {
                    McpError::internal_error(
                        format!("Failed to build memory graph: {}", e),
                        "memory_graph",
                    )
                })?
        };

        if graph.memories.is_empty() {
            return Ok(format!("Memory '{}' not found", memory_id));
        }

        // Format graph as text
        let mut output = format!(
            "📊 Memory Graph (root: {}, depth: {})\n\n",
            graph.root, depth
        );
        output.push_str(&format!("Memories: {}\n", graph.memories.len()));
        output.push_str(&format!("Relationships: {}\n\n", graph.relationships.len()));

        // List memories
        output.push_str("🧠 Memories in Graph:\n\n");
        for (id, memory) in graph.memories.iter().take(20) {
            // Limit for readability
            output.push_str(&format!("[{}]\n", id));
            output.push_str(&format!("  Title: {}\n", memory.title));
            output.push_str(&format!("  Type: {}\n", memory.memory_type));
            output.push_str(&format!(
                "  Created: {}\n",
                memory.created_at.format("%Y-%m-%d %H:%M")
            ));

            // Add snippet of content
            let content_preview = if memory.content.len() > 100 {
                format!("{}...", &memory.content[..100])
            } else {
                memory.content.clone()
            };
            output.push_str(&format!("  Content: {}\n\n", content_preview));
        }

        if graph.memories.len() > 20 {
            output.push_str(&format!(
                "... and {} more memories\n\n",
                graph.memories.len() - 20
            ));
        }

        // List relationships
        if !graph.relationships.is_empty() {
            output.push_str("🔗 Relationships:\n\n");
            for rel in graph.relationships.iter().take(30) {
                // Limit for readability
                output.push_str(&format!(
                    "  {} -> {} ({}, strength: {:.2})\n",
                    rel.source_id, rel.target_id, rel.relationship_type, rel.strength
                ));
            }

            if graph.relationships.len() > 30 {
                output.push_str(&format!(
                    "\n... and {} more relationships\n",
                    graph.relationships.len() - 30
                ));
            }
        }

        // Apply token truncation
        Ok(truncate_output(&output, max_tokens))
    }

    /// Execute the relate tool — manually create a typed relationship between two memories
    pub async fn execute_relate(&self, arguments: &Value) -> Result<String, McpError> {
        let source_id = arguments
            .get("source_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'source_id'", "relate")
            })?
            .to_string();

        let target_id = arguments
            .get("target_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'target_id'", "relate")
            })?
            .to_string();

        let rel_type_str = arguments
            .get("relationship_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("Missing required parameter 'relationship_type'", "relate")
            })?;

        let relationship_type = match rel_type_str {
            "related_to" => crate::memory::types::RelationshipType::RelatedTo,
            "depends_on" => crate::memory::types::RelationshipType::DependsOn,
            "supersedes" => crate::memory::types::RelationshipType::Supersedes,
            "similar" => crate::memory::types::RelationshipType::Similar,
            "conflicts" => crate::memory::types::RelationshipType::Conflicts,
            "implements" => crate::memory::types::RelationshipType::Implements,
            "extends" => crate::memory::types::RelationshipType::Extends,
            other => crate::memory::types::RelationshipType::Custom(other.to_string()),
        };

        let strength = arguments
            .get("strength")
            .and_then(|v| v.as_f64())
            .map(|v| (v as f32).clamp(0.0, 1.0))
            .unwrap_or(0.8);

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let res = {
            let mut manager_guard = self.memory_manager.lock().await;
            manager_guard
                .create_relationship(
                    source_id,
                    target_id,
                    relationship_type,
                    strength,
                    description,
                )
                .await
        };

        match res {
            Ok(rel) => Ok(format!(
                "✅ Relationship created\nID: {}\n{} -> {} ({}, strength: {:.2})",
                rel.id, rel.source_id, rel.target_id, rel.relationship_type, rel.strength
            )),
            Err(e) => Err(McpError::internal_error(
                format!("Failed to create relationship: {}", e),
                "relate",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryProvider;

    #[test]
    fn test_tool_definitions_unlocked_has_scope_props() {
        let tools = MemoryProvider::get_tool_definitions(false);

        // memorize, forget, relate are write tools — must expose project + role
        for write_tool in &["memorize", "forget", "relate"] {
            let tool = tools
                .iter()
                .find(|t| t.name == *write_tool)
                .unwrap_or_else(|| panic!("Tool '{}' not found in definitions", write_tool));
            let props = tool.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("Tool '{}' has no properties object", write_tool));
            assert!(
                props.contains_key("project"),
                "Unlocked write tool '{}' must expose 'project' param",
                write_tool
            );
            assert!(
                props.contains_key("role"),
                "Unlocked write tool '{}' must expose 'role' param",
                write_tool
            );
        }

        // remember, auto_link, memory_graph are read tools — must expose project + role
        for read_tool in &["remember", "auto_link", "memory_graph"] {
            let tool = tools
                .iter()
                .find(|t| t.name == *read_tool)
                .unwrap_or_else(|| panic!("Tool '{}' not found in definitions", read_tool));
            let props = tool.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("Tool '{}' has no properties object", read_tool));
            assert!(
                props.contains_key("project"),
                "Unlocked read tool '{}' must expose 'project' param",
                read_tool
            );
            assert!(
                props.contains_key("role"),
                "Unlocked read tool '{}' must expose 'role' param",
                read_tool
            );
        }
    }

    #[test]
    fn test_tool_definitions_locked_hides_scope_props() {
        let tools = MemoryProvider::get_tool_definitions(true);

        for tool_name in &[
            "memorize",
            "forget",
            "relate",
            "remember",
            "auto_link",
            "memory_graph",
        ] {
            let tool = tools
                .iter()
                .find(|t| t.name == *tool_name)
                .unwrap_or_else(|| panic!("Tool '{}' not found in definitions", tool_name));
            let props = tool.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("Tool '{}' has no properties object", tool_name));
            assert!(
                !props.contains_key("project"),
                "Locked tool '{}' must NOT expose 'project' param",
                tool_name
            );
            assert!(
                !props.contains_key("role"),
                "Locked tool '{}' must NOT expose 'role' param",
                tool_name
            );
        }
    }

    #[test]
    fn test_tool_definitions_count() {
        // Ensure we always have exactly 6 memory tools
        let tools = MemoryProvider::get_tool_definitions(false);
        assert_eq!(
            tools.len(),
            6,
            "Expected 6 memory tools, got {}",
            tools.len()
        );
    }

    #[test]
    fn test_tool_definitions_required_fields_unchanged_when_locked() {
        // Locking must not affect required fields — only hides optional scope params
        let locked = MemoryProvider::get_tool_definitions(true);
        let unlocked = MemoryProvider::get_tool_definitions(false);

        for tool_name in &["memorize", "remember", "forget"] {
            let t_locked = locked.iter().find(|t| t.name == *tool_name).unwrap();
            let t_unlocked = unlocked.iter().find(|t| t.name == *tool_name).unwrap();
            assert_eq!(
                t_locked.input_schema["required"], t_unlocked.input_schema["required"],
                "Required fields for '{}' must be identical regardless of locked state",
                tool_name
            );
        }
    }
}
