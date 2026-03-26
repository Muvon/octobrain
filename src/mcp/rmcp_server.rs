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

//! MCP Server implementation using the official rmcp SDK
//! Provides full MCP 2025-11-25 protocol compliance with stdio and HTTP transports

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters, ServerHandler},
    model::{
        Implementation, InitializeRequestParams, InitializeResult, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpService,
    },
    ErrorData as McpError, RoleServer, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::Config;
use crate::mcp::knowledge::KnowledgeProvider;
use crate::mcp::memory::MemoryProvider;

/// Session state for project/role locking from MCP capabilities
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub project: Option<String>,
    pub role: Option<String>,
    pub locked: bool,
}

/// MCP Server using rmcp SDK
#[derive(Clone)]
pub struct OctobrainServer {
    config: Config,
    working_directory: std::path::PathBuf,
    memory: Arc<Mutex<Option<MemoryProvider>>>,
    knowledge: Arc<Mutex<Option<KnowledgeProvider>>>,
    session: Arc<Mutex<SessionState>>,
    tool_router: ToolRouter<Self>,
}

impl OctobrainServer {
    pub fn new(config: Config, working_directory: std::path::PathBuf) -> Self {
        Self {
            config,
            working_directory,
            memory: Arc::new(Mutex::new(None)),
            knowledge: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(SessionState::default())),
            tool_router: Self::tool_router(),
        }
    }

    /// Get or initialize memory provider with session state
    async fn get_or_init_memory(&self) -> Result<MemoryProvider, McpError> {
        let session = self.session.lock().await.clone();

        // Check if already initialized
        {
            let guard = self.memory.lock().await;
            if let Some(provider) = guard.as_ref() {
                return Ok(provider.clone());
            }
        }

        // Initialize with session project/role
        let mut guard = self.memory.lock().await;
        if let Some(provider) = guard.as_ref() {
            return Ok(provider.clone());
        }

        let provider = MemoryProvider::new(
            &self.config,
            self.working_directory.clone(),
            session.project.clone(),
            session.role.clone(),
        )
        .await
        .map_err(|e| {
            McpError::internal_error(format!("Failed to initialize memory: {}", e), None)
        })?;

        *guard = Some(provider.clone());
        Ok(provider)
    }

    /// Get or initialize knowledge provider
    async fn get_or_init_knowledge(&self) -> Result<KnowledgeProvider, McpError> {
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

        let provider = KnowledgeProvider::new(&self.config).await.map_err(|e| {
            McpError::internal_error(format!("Failed to initialize knowledge: {}", e), None)
        })?;

        *guard = Some(provider.clone());
        Ok(provider)
    }

    /// Run server using stdio transport
    pub async fn run_stdio(self) -> Result<()> {
        let transport = rmcp::transport::stdio();

        self.serve(transport)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize MCP server: {}", e))?
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP server task failed: {}", e))?;

        Ok(())
    }

    /// Run server using HTTP transport (streamable HTTP for MCP 2025-11-25)
    pub async fn run_http(self, bind_addr: &str) -> Result<()> {
        use axum::Router;
        use tower_http::cors::{Any, CorsLayer};

        let addr = bind_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|e| anyhow::anyhow!("Invalid bind address '{}': {}", bind_addr, e))?;

        let config = self.config.clone();
        let working_directory = self.working_directory.clone();

        let service = StreamableHttpService::new(
            move || {
                Ok(OctobrainServer::new(
                    config.clone(),
                    working_directory.clone(),
                ))
            },
            LocalSessionManager::default().into(),
            Default::default(),
        );

        let app = Router::new().nest_service("/mcp", service).layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([http::Method::POST, http::Method::GET, http::Method::OPTIONS])
                .allow_headers(Any),
        );

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", addr, e))?;

        debug!("MCP HTTP server listening on {}", addr);

        axum::serve(listener, app)
            .await
            .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))?;

        Ok(())
    }
}

// ============================================================================
// Tool parameter schemas using rmcp macros
// ============================================================================

/// Memorize tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorizeParams {
    /// Short, descriptive title for the memory (5-200 characters)
    pub title: String,
    /// Detailed content to remember
    pub content: String,
    /// Category of memory for better organization
    pub memory_type: Option<String>,
    /// Importance score from 0.0 to 1.0 (higher = more important)
    pub importance: Option<f32>,
    /// Tags for categorization
    pub tags: Option<Vec<String>>,
    /// Related file paths
    pub related_files: Option<Vec<String>>,
    /// Trust tier: 'user_confirmed' or 'agent_inferred'
    pub source: Option<String>,
    /// Project key to scope this memory to
    pub project: Option<String>,
    /// Role tag to attach to this memory
    pub role: Option<String>,
}

/// Remember tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RememberParams {
    /// Search query: a string or array of strings for comprehensive search
    pub query: Option<serde_json::Value>,
    /// Filter by memory types
    pub memory_types: Option<Vec<String>>,
    /// Filter by tags
    pub tags: Option<Vec<String>>,
    /// Filter by related files
    pub related_files: Option<Vec<String>>,
    /// Maximum number of memories to return
    pub limit: Option<usize>,
    /// Minimum relevance score (0.0-1.0)
    pub min_relevance: Option<f32>,
    /// Project key filter
    pub project: Option<String>,
    /// Role filter
    pub role: Option<String>,
}

/// Forget tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgetParams {
    /// Memory ID to forget
    pub memory_id: Option<String>,
    /// Query to find memories to forget
    pub query: Option<String>,
    /// Filter by memory types when using query
    pub memory_types: Option<Vec<String>>,
    /// Filter by tags when using query
    pub tags: Option<Vec<String>>,
    /// Confirm deletion without prompting
    pub confirm: Option<bool>,
    /// Project key filter
    pub project: Option<String>,
    /// Role filter
    pub role: Option<String>,
}

/// Auto-link tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AutoLinkParams {
    /// Memory ID to auto-link
    pub memory_id: String,
}

/// Memory graph tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryGraphParams {
    /// Root memory ID
    pub memory_id: String,
    /// Depth of graph traversal (1-3 recommended)
    pub depth: Option<usize>,
}

/// Relate tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelateParams {
    /// Source memory ID
    pub source_id: String,
    /// Target memory ID
    pub target_id: String,
    /// Relationship type
    pub relationship_type: Option<String>,
    /// Relationship strength (0.0-1.0)
    pub strength: Option<f32>,
    /// Description of relationship
    pub description: Option<String>,
}

/// Knowledge search tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeSearchParams {
    /// What to search for
    pub query: String,
    /// URL to fetch and index before searching
    pub source_url: Option<String>,
}

// ============================================================================
// Tool implementations using rmcp macros
// ============================================================================

#[tool_router]
impl OctobrainServer {
    #[tool(
        name = "memorize",
        description = "Store information, insights, or context in memory. Call remember first to avoid duplicates. Set source='user_confirmed' for user-stated facts (importance 0.8-1.0), 'agent_inferred' for AI conclusions (0.3-0.6). Skip transient state or things easily re-derived."
    )]
    async fn memorize(
        &self,
        Parameters(params): Parameters<MemorizeParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let mut args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        // Handle session locking
        let session = self.session.lock().await;
        if session.locked {
            // Remove project/role from args if locked
            if let Some(obj) = args.as_object_mut() {
                obj.remove("project");
                obj.remove("role");
            }
        }

        provider.execute_memorize(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "remember",
        description = "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. Prefer 2-5 related query terms for broader coverage."
    )]
    async fn remember(
        &self,
        Parameters(params): Parameters<RememberParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let mut args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        // Handle session locking
        let session = self.session.lock().await;
        if session.locked {
            if let Some(obj) = args.as_object_mut() {
                obj.remove("project");
                obj.remove("role");
            }
        }

        provider.execute_remember(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "forget",
        description = "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal."
    )]
    async fn forget(
        &self,
        Parameters(params): Parameters<ForgetParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let mut args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        let session = self.session.lock().await;
        if session.locked {
            if let Some(obj) = args.as_object_mut() {
                obj.remove("project");
                obj.remove("role");
            }
        }

        provider.execute_forget(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "auto_link",
        description = "Find and connect semantically similar memories for a given memory ID. Auto-linking runs on new memories automatically — call this manually to refresh links."
    )]
    async fn auto_link(
        &self,
        Parameters(params): Parameters<AutoLinkParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        provider.execute_auto_link(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "memory_graph",
        description = "Retrieve a memory and its connected neighbors as a graph. remember already includes 1-hop neighbors — use this only for deeper traversal (depth > 1)."
    )]
    async fn memory_graph(
        &self,
        Parameters(params): Parameters<MemoryGraphParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        provider.execute_memory_graph(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "relate",
        description = "Create a typed relationship between two memories. Use when auto-linking missed a meaningful connection or you need a specific type."
    )]
    async fn relate(
        &self,
        Parameters(params): Parameters<RelateParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_memory().await?;

        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        provider.execute_relate(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }

    #[tool(
        name = "knowledge_search",
        description = "Search indexed web knowledge semantically. Provide source_url to fetch and index a page on-the-fly, then search its content. Omit source_url to search across all previously indexed pages."
    )]
    async fn knowledge_search(
        &self,
        Parameters(params): Parameters<KnowledgeSearchParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_knowledge().await?;

        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;

        provider.execute_knowledge_search(&args).await.map_err(|e| {
            McpError::internal_error(
                e.message,
                Some(serde_json::to_value(e.operation).unwrap_or_default()),
            )
        })
    }
}

// ============================================================================
// ServerHandler implementation
// ============================================================================

#[tool_handler]
impl ServerHandler for OctobrainServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "octobrain".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: Some("Octobrain Memory Server".to_string()),
                description: Some(
                    "Standalone memory management system for AI context and conversation state"
                        .to_string(),
                ),
                website_url: None,
                icons: None,
            },
            instructions: Some(
                "This server provides memory tools for storing and retrieving AI context. \
                 Use 'memorize' to store information, 'remember' for semantic search, \
                 'forget' to delete memories, 'auto_link' to find related memories, \
                 'memory_graph' to explore memory connections, 'relate' to create relationships, \
                 and 'knowledge_search' to search indexed web content."
                    .to_string(),
            ),
        }
    }

    /// Extract project/role from experimental capabilities during initialize handshake
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // Extract session from capabilities.experimental.session
        if let Some(experimental) = &request.capabilities.experimental {
            if let Some(session_obj) = experimental.get("session") {
                let project = session_obj
                    .get("project")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let role = session_obj
                    .get("role")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);

                let mut session = self.session.lock().await;
                session.project = project;
                session.role = role;
                session.locked = true;

                debug!(
                    "Session locked: project={:?}, role={:?}",
                    session.project, session.role
                );
            }
        }

        // Store peer info and return server info (default behavior)
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }
}
