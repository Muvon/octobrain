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

//! HTTP server implementation for MCP over HTTP
//! Provides HTTP transport layer for MCP protocol as an alternative to stdin/stdout

use anyhow::Result;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::debug;

use super::knowledge::KnowledgeProvider;
use super::memory::MemoryProvider;
use super::types::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpError};

const MCP_MAX_REQUEST_SIZE: usize = 10_485_760; // 10MB

/// Shared state for HTTP MCP handlers
pub struct HttpServerState {
    pub memory: Option<MemoryProvider>,
    pub knowledge: Option<KnowledgeProvider>,
}

/// Handle a single HTTP connection
pub async fn handle_http_connection(
    mut stream: TcpStream,
    state: Arc<Mutex<HttpServerState>>,
) -> Result<()> {
    let mut buffer = vec![0; 8192];
    let bytes_read = stream.read(&mut buffer).await?;

    if bytes_read == 0 {
        return Ok(());
    }

    let request_str = String::from_utf8_lossy(&buffer[..bytes_read]);
    let mut lines = request_str.lines();
    let request_line = lines.next().unwrap_or("");

    if !request_line.starts_with("POST /mcp") && !request_line.starts_with("POST / ") {
        let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    let mut content_length = 0;
    let mut body_start = 0;

    for (i, line) in lines.enumerate() {
        if line.is_empty() {
            let lines_before_body: Vec<&str> = request_str.lines().take(i + 2).collect();
            body_start = lines_before_body.join("\n").len() + 1;
            break;
        }
        if line.to_lowercase().starts_with("content-length:") {
            if let Some(len_str) = line.split(':').nth(1) {
                content_length = len_str.trim().parse().unwrap_or(0);
            }
        }
    }

    let json_body = if content_length > 0 && body_start < bytes_read {
        let body_bytes =
            &buffer[body_start..std::cmp::min(body_start + content_length, bytes_read)];
        String::from_utf8_lossy(body_bytes).to_string()
    } else {
        return send_http_error(&mut stream, 400, "Missing or invalid request body").await;
    };

    let request: JsonRpcRequest = match serde_json::from_str(&json_body) {
        Ok(req) => req,
        Err(e) => {
            debug!("Failed to parse JSON-RPC request: {}", e);
            return send_http_error(&mut stream, 400, "Invalid JSON-RPC request").await;
        }
    };

    let request_id = request.id.clone();

    let response = match request.method.as_str() {
        "initialize" => handle_initialize(&request),
        "tools/list" => handle_tools_list(&request),
        "ping" => handle_ping(&request),
        "tools/call" => handle_tools_call(&request, &state).await,
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        },
    };

    // Notifications (no id) must not receive a JSON-RPC response
    if request_id.is_none() {
        return send_http_no_content(&mut stream).await;
    }

    send_http_response(&mut stream, &response).await
}

fn handle_initialize(request: &JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: request.id.clone(),
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
    }
}

fn handle_tools_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    let mut tools = MemoryProvider::get_tool_definitions();
    tools.extend(KnowledgeProvider::get_tool_definitions());
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: request.id.clone(),
        result: Some(json!({ "tools": tools })),
        error: None,
    }
}

fn handle_ping(request: &JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: request.id.clone(),
        result: Some(json!({})),
        error: None,
    }
}

async fn handle_tools_call(
    request: &JsonRpcRequest,
    state: &Arc<Mutex<HttpServerState>>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Invalid params: missing parameters object".to_string(),
                    data: None,
                }),
            };
        }
    };

    let tool_name = match params.get("name").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Invalid params: missing tool name".to_string(),
                    data: None,
                }),
            };
        }
    };

    let default_args = json!({});
    let arguments = params.get("arguments").unwrap_or(&default_args);

    if let Ok(args_str) = serde_json::to_string(arguments) {
        if args_str.len() > MCP_MAX_REQUEST_SIZE {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: "Tool arguments too large".to_string(),
                    data: Some(json!({ "max_size": MCP_MAX_REQUEST_SIZE })),
                }),
            };
        }
    }

    let is_memory_tool = matches!(
        tool_name,
        "memorize" | "remember" | "forget" | "auto_link" | "memory_graph" | "relate"
    );
    let is_knowledge_tool = tool_name == "knowledge_search";

    let result = if is_memory_tool {
        let provider = {
            let guard = state.lock().await;
            guard.memory.clone()
        };
        let provider = match provider {
            Some(p) => p,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(
                        McpError::internal_error("Memory provider not initialized", "tools/call")
                            .into_jsonrpc(),
                    ),
                };
            }
        };
        match tool_name {
            "memorize" => provider.execute_memorize(arguments).await,
            "remember" => provider.execute_remember(arguments).await,
            "forget" => provider.execute_forget(arguments).await,
            "auto_link" => provider.execute_auto_link(arguments).await,
            "memory_graph" => provider.execute_memory_graph(arguments).await,
            "relate" => provider.execute_relate(arguments).await,
            _ => unreachable!(),
        }
    } else if is_knowledge_tool {
        let provider = {
            let guard = state.lock().await;
            guard.knowledge.clone()
        };
        let provider = match provider {
            Some(p) => p,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: None,
                    error: Some(
                        McpError::internal_error(
                            "Knowledge provider not initialized",
                            "tools/call",
                        )
                        .into_jsonrpc(),
                    ),
                };
            }
        };
        provider.execute_knowledge_search(arguments).await
    } else {
        return JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: None,
            error: Some(
                McpError::method_not_found(format!("Unknown tool: {}", tool_name), "tools/call")
                    .into_jsonrpc(),
            ),
        };
    };

    match result {
        Ok(content) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(json!({
                "content": [{ "type": "text", "text": content }]
            })),
            error: None,
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: None,
            error: Some(e.into_jsonrpc()),
        },
    }
}

pub async fn send_http_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    let status_text = match status {
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let response = format!(
		"HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
		status, status_text, message.len(), message
	);
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

pub async fn send_http_response(stream: &mut TcpStream, response: &JsonRpcResponse) -> Result<()> {
    let json_response = serde_json::to_string(response)?;
    let http_response = format!(
		"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
		json_response.len(),
		json_response
	);
    stream.write_all(http_response.as_bytes()).await?;
    Ok(())
}

pub async fn send_http_no_content(stream: &mut TcpStream) -> Result<()> {
    let response = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n\r\n";
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}
