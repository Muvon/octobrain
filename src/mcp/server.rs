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
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::Config;
use crate::mcp::http::{handle_http_connection, HttpServerState};
use crate::mcp::knowledge::KnowledgeProvider;
use crate::mcp::memory::MemoryProvider;
use crate::mcp::types::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

/// Simplified MCP Server for memory tools only
pub struct McpServer {
    memory: tokio::sync::Mutex<Option<MemoryProvider>>,
    knowledge: tokio::sync::Mutex<Option<KnowledgeProvider>>,
    config: Config,
    working_directory: std::path::PathBuf,
    /// Project key locked from MCP capabilities on initialize (None = auto-detect)
    session_project: tokio::sync::Mutex<Option<String>>,
    /// Role locked from MCP capabilities on initialize (None = no filter)
    session_role: tokio::sync::Mutex<Option<String>>,
    /// True once capabilities were provided in initialize — locks project/role for the session
    capabilities_locked: tokio::sync::Mutex<bool>,
}

impl McpServer {
    pub async fn new(config: Config, working_directory: std::path::PathBuf) -> Result<Self> {
        Ok(Self {
            memory: tokio::sync::Mutex::new(None),
            knowledge: tokio::sync::Mutex::new(None),
            config,
            working_directory,
            session_project: tokio::sync::Mutex::new(None),
            session_role: tokio::sync::Mutex::new(None),
            capabilities_locked: tokio::sync::Mutex::new(false),
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
            "initialize" => {
                // Extract optional project/role from experimental capabilities
                // e.g. params.capabilities.experimental.session.project
                let session = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("capabilities"))
                    .and_then(|c| c.get("experimental"))
                    .and_then(|e| e.get("session"));

                if let Some(sess) = session {
                    let project = sess
                        .get("project")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    let role = sess
                        .get("role")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    *self.session_project.lock().await = project;
                    *self.session_role.lock().await = role;
                    *self.capabilities_locked.lock().await = true;
                }

                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {
                                "listChanged": false
                            }
                        },
                        "serverInfo": {
                            "name": "octobrain",
                            "version": env!("CARGO_PKG_VERSION"),
                            "description": "Standalone memory management system for AI context and conversation state"
                        },
                        "instructions": "This server provides memory tools for storing and retrieving AI context. Use 'memorize' to store information, 'remember' for semantic search, 'forget' to delete memories, 'auto_link' to find related memories, 'memory_graph' to explore memory connections, and 'knowledge_search' to search indexed web content."
                    })),
                    error: None,
                })
            }

            "tools/list" => {
                let locked = *self.capabilities_locked.lock().await;
                let mut tools = MemoryProvider::get_tool_definitions(locked);
                tools.extend(KnowledgeProvider::get_tool_definitions());
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

                // Check if it's a memory or knowledge tool
                let is_memory_tool = matches!(
                    tool_name,
                    "memorize" | "remember" | "forget" | "auto_link" | "memory_graph" | "relate"
                );
                let is_knowledge_tool = tool_name == "knowledge_search";

                let response = if is_memory_tool {
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
                        "relate" => memory.execute_relate(&arguments).await,
                        _ => unreachable!(),
                    };

                    match result {
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
                    }
                } else if is_knowledge_tool {
                    let knowledge = match self.get_or_init_knowledge().await {
                        Ok(knowledge) => knowledge,
                        Err(err) => {
                            return Some(JsonRpcResponse {
                                jsonrpc: "2.0".to_string(),
                                id,
                                result: None,
                                error: Some(err.into_jsonrpc()),
                            });
                        }
                    };

                    let result = knowledge.execute_knowledge_search(&arguments).await;

                    match result {
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
                    }
                } else {
                    JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: None,
                        error: Some(
                            crate::mcp::types::McpError::method_not_found(
                                format!("Unknown tool: {}", tool_name),
                                "tools/call",
                            )
                            .into_jsonrpc(),
                        ),
                    }
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

        let project = self.session_project.lock().await.clone();
        let role = self.session_role.lock().await.clone();
        let provider =
            MemoryProvider::new(&self.config, self.working_directory.clone(), project, role)
                .await?;
        *guard = Some(provider.clone());
        Ok(provider)
    }

    async fn get_or_init_knowledge(
        &self,
    ) -> Result<KnowledgeProvider, crate::mcp::types::McpError> {
        {
            let guard = self.knowledge.lock().await;
            if let Some(provider) = guard.as_ref() {
                return Ok(provider.clone());
            }
        }

        let mut guard = self.knowledge.lock().await;
        if let Some(provider) = guard.as_ref() {
            return Ok(provider.clone());
        }

        let provider = KnowledgeProvider::new(&self.config).await?;
        *guard = Some(provider.clone());
        Ok(provider)
    }

    /// Run the MCP server over HTTP instead of stdin/stdout
    pub async fn run_http(&self, bind_addr: &str) -> Result<()> {
        let addr = bind_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|e| anyhow::anyhow!("Invalid bind address '{}': {}", bind_addr, e))?;

        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", addr, e))?;

        debug!("MCP HTTP server listening on {}", addr);

        // Pre-initialize providers so HTTP connections don't race on first request
        let memory = self.get_or_init_memory().await.ok();
        let knowledge = self.get_or_init_knowledge().await.ok();

        let state = std::sync::Arc::new(Mutex::new(HttpServerState { memory, knowledge }));

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_http_connection(stream, state).await {
                            debug!("HTTP connection error from {}: {}", peer_addr, e);
                        }
                    });
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("HTTP server accept error: {}", e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::McpServer;
    use crate::config::Config;
    use crate::mcp::types::{JsonRpcRequest, JsonRpcResponse};
    use serde_json::json;

    fn make_server() -> McpServer {
        let config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
            .expect("default.toml must be valid");
        let working_directory = std::env::current_dir().unwrap();
        McpServer {
            memory: tokio::sync::Mutex::new(None),
            knowledge: tokio::sync::Mutex::new(None),
            config,
            working_directory,
            session_project: tokio::sync::Mutex::new(None),
            session_role: tokio::sync::Mutex::new(None),
            capabilities_locked: tokio::sync::Mutex::new(false),
        }
    }

    fn make_initialize_request(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: Some(params),
        }
    }

    #[tokio::test]
    async fn test_initialize_without_capabilities_stays_unlocked() {
        let server = make_server();
        let req = make_initialize_request(json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "test", "version": "1.0" }
        }));

        let _resp = server.handle_request(req).await;

        assert!(
            !*server.capabilities_locked.lock().await,
            "No session caps → should stay unlocked"
        );
        assert!(server.session_project.lock().await.is_none());
        assert!(server.session_role.lock().await.is_none());
    }

    #[tokio::test]
    async fn test_initialize_with_project_and_role_locks_session() {
        let server = make_server();
        let req = make_initialize_request(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "experimental": {
                    "session": {
                        "project": "abc123",
                        "role": "developer"
                    }
                }
            }
        }));

        let _resp = server.handle_request(req).await;

        assert!(
            *server.capabilities_locked.lock().await,
            "Session caps provided → must be locked"
        );
        assert_eq!(
            server.session_project.lock().await.as_deref(),
            Some("abc123")
        );
        assert_eq!(
            server.session_role.lock().await.as_deref(),
            Some("developer")
        );
    }

    #[tokio::test]
    async fn test_initialize_with_project_only_locks_session() {
        let server = make_server();
        let req = make_initialize_request(json!({
            "capabilities": {
                "experimental": {
                    "session": { "project": "myproject" }
                }
            }
        }));

        let _resp = server.handle_request(req).await;

        assert!(*server.capabilities_locked.lock().await);
        assert_eq!(
            server.session_project.lock().await.as_deref(),
            Some("myproject")
        );
        assert!(
            server.session_role.lock().await.is_none(),
            "Role not provided → must be None"
        );
    }

    #[tokio::test]
    async fn test_initialize_with_role_only_locks_session() {
        let server = make_server();
        let req = make_initialize_request(json!({
            "capabilities": {
                "experimental": {
                    "session": { "role": "reviewer" }
                }
            }
        }));

        let _resp = server.handle_request(req).await;

        assert!(*server.capabilities_locked.lock().await);
        assert!(
            server.session_project.lock().await.is_none(),
            "Project not provided → must be None"
        );
        assert_eq!(
            server.session_role.lock().await.as_deref(),
            Some("reviewer")
        );
    }

    #[tokio::test]
    async fn test_initialize_returns_valid_jsonrpc_response() {
        let server = make_server();
        let req = make_initialize_request(json!({}));
        let resp: Option<JsonRpcResponse> = server.handle_request(req).await;

        let resp = resp.expect("initialize must return a response");
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.error.is_none(), "initialize must not return an error");
        let result = resp.result.expect("initialize must have a result");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["serverInfo"]["name"].as_str().is_some());
    }
}
