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

/// Error type for memory and knowledge provider operations.
/// Carries a human-readable message, the operation name for context, and optional details.
#[derive(Debug, Clone)]
pub struct McpError {
    pub message: String,
    pub operation: String,
    pub details: Option<String>,
}

impl McpError {
    pub fn invalid_params(message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            operation: operation.into(),
            details: None,
        }
    }

    pub fn internal_error(message: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            operation: operation.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
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
