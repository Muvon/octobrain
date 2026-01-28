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
use serde_json::json;
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::debug;

use crate::config::Config;
use crate::mcp::memory::MemoryProvider;
use crate::mcp::types::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

/// Simplified MCP Server for memory tools only
pub struct McpServer {
    memory: tokio::sync::Mutex<Option<MemoryProvider>>,
    config: Config,
    working_directory: std::path::PathBuf,
}

impl McpServer {
    pub async fn new(config: Config, working_directory: std::path::PathBuf) -> Result<Self> {
        Ok(Self {
            memory: tokio::sync::Mutex::new(None),
            config,
            working_directory,
        })
    }
    /// Run the MCP server on stdio
    pub async fn run(&self) -> Result<()> {
        let stdin = stdin();
        let mut stdout = stdout();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;

            if bytes_read == 0 {
                debug!("EOF received, shutting down");
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            debug!("Received request: {}", trimmed);

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
                Ok(req) => req,
                Err(e) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };
                    let response_json = serde_json::to_string(&error_response)?;
                    stdout.write_all(response_json.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                    continue;
                }
            };

            // Handle request
            let response = self.handle_request(request).await;

            // Send response
            if let Some(response) = response {
                let response_json = serde_json::to_string(&response)?;
                stdout.write_all(response_json.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }

        Ok(())
    }

    async fn handle_request(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id.clone();
        let has_id = id.is_some();

        match request.method.as_str() {
            "initialize" => Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "octobrain",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            }),

            "tools/list" => {
                let tools = MemoryProvider::get_tool_definitions();
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(json!({ "tools": tools })),
                    error: None,
                })
            }

            "tools/call" => {
                let params = request.params.unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                let memory = match self.get_or_init_memory().await {
                    Ok(memory) => memory,
                    Err(err) => {
                        return Some(JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id,
                            result: None,
                            error: Some(err.into_jsonrpc()),
                        });
                    }
                };

                let result = match tool_name {
                    "memorize" => memory.execute_memorize(&arguments).await,
                    "remember" => memory.execute_remember(&arguments).await,
                    "forget" => memory.execute_forget(&arguments).await,
                    "auto_link" => memory.execute_auto_link(&arguments).await,
                    "memory_graph" => memory.execute_memory_graph(&arguments).await,
                    _ => Err(crate::mcp::types::McpError::method_not_found(
                        format!("Unknown tool: {}", tool_name),
                        "tools/call",
                    )),
                };

                let response = match result {
                    Ok(content) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: Some(json!({
                            "content": [{
                                "type": "text",
                                "text": content
                            }]
                        })),
                        error: None,
                    },
                    Err(e) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: None,
                        error: Some(e.into_jsonrpc()),
                    },
                };
                Some(response)
            }

            _ => {
                if !has_id {
                    // Notification: no response required
                    None
                } else {
                    Some(JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32601,
                            message: format!("Method not found: {}", request.method),
                            data: None,
                        }),
                    })
                }
            }
        }
    }

    async fn get_or_init_memory(&self) -> Result<MemoryProvider, crate::mcp::types::McpError> {
        {
            let guard = self.memory.lock().await;
            if let Some(provider) = guard.as_ref() {
                return Ok(provider.clone());
            }
        }

        let mut guard = self.memory.lock().await;
        if let Some(provider) = guard.as_ref() {
            return Ok(provider.clone());
        }

        let provider = MemoryProvider::new(&self.config, self.working_directory.clone()).await?;
        *guard = Some(provider.clone());
        Ok(provider)
    }
}
