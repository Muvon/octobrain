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
pub struct McpServer {
    config: Config,
    working_directory: std::path::PathBuf,
    memory: Arc<Mutex<Option<MemoryProvider>>>,
    knowledge: Arc<Mutex<Option<KnowledgeProvider>>>,
    session: Arc<Mutex<SessionState>>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
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
            move || Ok(McpServer::new(config.clone(), working_directory.clone())),
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
// Shared enum types for schema constraints
// ============================================================================

/// Memory category for organization and filtering
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Code,
    Architecture,
    BugFix,
    Feature,
    Documentation,
    UserPreference,
    Decision,
    Learning,
    Configuration,
    Testing,
    Performance,
    Security,
    Insight,
}

/// Trust tier for memory source attribution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceTrust {
    /// User explicitly stated or approved this fact
    UserConfirmed,
    /// AI-inferred conclusion
    AgentInferred,
}

/// Relationship type between memories
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    /// General association
    RelatedTo,
    /// A needs B
    DependsOn,
    /// A replaces B
    Supersedes,
    /// Near-duplicate
    Similar,
    /// Contradicts
    Conflicts,
    /// Concrete implementation of abstract concept
    Implements,
    /// Builds on top of
    Extends,
}

/// Search query: either a single string or an array of strings for broader coverage
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum QueryInput {
    /// Single semantic search query
    Single(String),
    /// 2-5 related terms for comprehensive coverage — preferred over single query
    Multiple(Vec<String>),
}

// ============================================================================
// Tool parameter schemas using rmcp macros
// ============================================================================

/// Memorize tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorizeParams {
    /// Short descriptive title
    pub title: String,
    /// Full content — explanations, code snippets, decisions, etc.
    pub content: String,
    /// Memory category
    pub memory_type: Option<MemoryType>,
    /// Importance 0.0-1.0: user facts 0.8-1.0, decisions 0.7-0.9, bug fixes 0.6-0.8, inferences 0.3-0.6
    #[schemars(range(min = 0.0, max = 1.0))]
    pub importance: Option<f32>,
    /// Tags for categorization and filtering
    #[schemars(length(max = 10))]
    pub tags: Option<Vec<String>>,
    /// File paths related to this memory
    #[schemars(length(max = 20))]
    pub related_files: Option<Vec<String>>,
    /// Trust tier: 'user_confirmed' (user explicitly stated/approved) ranks higher in retrieval; 'agent_inferred' for AI conclusions
    pub source: Option<SourceTrust>,
    /// Project key to scope this memory to. Defaults to auto-detected Git remote hash.
    pub project: Option<String>,
    /// Role tag to attach to this memory (e.g. 'developer', 'reviewer').
    pub role: Option<String>,
}

/// Remember tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RememberParams {
    /// String or array of 2-5 related terms. Array preferred for broader semantic coverage.
    pub query: QueryInput,
    /// Narrow results to specific memory categories
    pub memory_types: Option<Vec<MemoryType>>,
    /// Filter by tags
    pub tags: Option<Vec<String>>,
    /// Filter by related file paths
    pub related_files: Option<Vec<String>>,
    /// Max memories to return
    #[schemars(range(min = 1, max = 5))]
    pub limit: Option<usize>,
    /// Minimum relevance score (0.0-1.0)
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_relevance: Option<f32>,
    /// Filter by project key. If omitted, returns memories from all projects.
    pub project: Option<String>,
    /// Filter by role. If omitted, returns memories for all roles.
    pub role: Option<String>,
}

/// Forget tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgetParams {
    /// ID of memory to delete (from remember results)
    pub memory_id: Option<String>,
    /// Semantic query to find memories to delete (alternative to memory_id)
    pub query: Option<String>,
    /// Filter by memory types when using query
    pub memory_types: Option<Vec<MemoryType>>,
    /// Filter by tags when using query
    pub tags: Option<Vec<String>>,
    /// Must be true — deletion is permanent
    pub confirm: bool,
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
    /// Traversal depth (default 2; use 3+ for broad exploration)
    #[schemars(range(min = 1, max = 5))]
    pub depth: Option<usize>,
}

/// Relate tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelateParams {
    /// Source memory ID
    pub source_id: String,
    /// Target memory ID
    pub target_id: String,
    /// Relationship type: related_to (general), depends_on (A needs B), supersedes (A replaces B), similar (near-duplicate), conflicts (contradicts), implements (concrete of abstract), extends (builds on)
    pub relationship_type: RelationshipKind,
    /// Relationship strength 0.0-1.0
    #[schemars(range(min = 0.0, max = 1.0))]
    pub strength: Option<f32>,
    /// Why these memories are related
    #[schemars(length(max = 500))]
    pub description: Option<String>,
}

/// Knowledge search tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeSearchParams {
    /// What to search for, in natural language
    #[schemars(length(min = 3, max = 500))]
    pub query: String,
    /// Webpage URL to fetch, index, and search. If omitted, searches all previously indexed pages.
    pub source_url: Option<String>,
}

// ============================================================================
// Tool implementations using rmcp macros
// ============================================================================

#[tool_router]
impl McpServer {
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
        description = "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. Prefer 2-5 related query terms for broader coverage. Results show [CONFIRMED]/[INFERRED] trust labels."
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
        description = "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal. Don't forget memories just because they're old — importance decay handles that. Only delete when information is wrong or superseded."
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
        description = "Find and connect semantically similar memories for a given memory ID. Auto-linking runs on new memories automatically — call this manually to refresh links after content updates or for memories created outside normal flow."
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
        description = "Retrieve a memory and its connected neighbors as a graph. remember already includes 1-hop neighbors — use this only for deeper traversal (depth > 1) or to see the full relationship structure."
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
        description = "Create a typed relationship between two memories. Use when auto-linking missed a meaningful connection or you need a specific type. Types: related_to, depends_on, supersedes, similar, conflicts, implements, extends. Strength 0.9+ = strong, 0.5-0.8 = moderate, <0.5 = weak."
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
        description = "Search indexed web knowledge semantically. Provide source_url to fetch and index a page on-the-fly, then search its content. Omit source_url to search across all previously indexed pages. Not for general web search — use when you have a specific URL or want to query already-indexed content."
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
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            .with_server_info(
                Implementation::new("octobrain", env!("CARGO_PKG_VERSION"))
                    .with_title("Octobrain Memory Server")
                    .with_description(
                        "Standalone memory management system for AI context and conversation state",
                    ),
            )
            .with_instructions(
                "This server provides memory tools for storing and retrieving AI context. \
                 Use 'memorize' to store information, 'remember' for semantic search, \
                 'forget' to delete memories, 'auto_link' to find related memories, \
                 'memory_graph' to explore memory connections, 'relate' to create relationships, \
                 and 'knowledge_search' to search indexed web content.",
            )
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
