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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// MCP Protocol types
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// MCP-compliant error builder for consistent error handling
#[derive(Debug, Clone)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    pub operation: String,
    pub details: Option<String>,
}

impl McpError {
    /// Create a new MCP error
    pub fn new(code: i32, message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            operation: operation.into(),
            details: None,
        }
    }

    /// Add details to the error
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Convert to JsonRpcError for direct response
    pub fn into_jsonrpc(self) -> JsonRpcError {
        JsonRpcError {
            code: self.code,
            message: self.message.clone(),
            data: Some(json!({
                "operation": self.operation,
                "details": self.details.unwrap_or_default(),
                "error_type": match self.code {
                    -32602 => "invalid_params",
                    -32601 => "method_not_found",
                    -32603 => "internal_error",
                    -32600 => "invalid_request",
                    _ => "application_error"
                }
            })),
        }
    }

    /// Common error types for convenience
    pub fn invalid_params(message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self::new(-32602, message, operation)
    }

    pub fn internal_error(message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self::new(-32603, message, operation)
    }

    pub fn method_not_found(message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self::new(-32601, message, operation)
    }
}

impl From<anyhow::Error> for McpError {
    fn from(error: anyhow::Error) -> Self {
        McpError::internal_error(error.to_string(), "unknown_operation")
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.operation)
    }
}

impl std::error::Error for McpError {}

/// MCP Tool definitions
#[derive(Debug, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}
