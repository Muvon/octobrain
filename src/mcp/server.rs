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
//! Provides MCP 2025-03-26 protocol compliance with stdio and HTTP transports

use anyhow::Result;
use rmcp::{
    handler::server::{wrapper::Parameters, ServerHandler},
    model::{
        Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
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
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tracing::debug;

/// Tools with project+role stripped — built once.
static TOOLS_LOCKED: OnceLock<Vec<Tool>> = OnceLock::new();
/// Tools with only role stripped — built once.
static TOOLS_ROLE_ONLY: OnceLock<Vec<Tool>> = OnceLock::new();
/// Full tools list — built once.
static TOOLS_FULL: OnceLock<Vec<Tool>> = OnceLock::new();

fn tools_full() -> &'static Vec<Tool> {
    TOOLS_FULL.get_or_init(|| McpServer::tool_router().list_all())
}

fn strip_fields(fields: &[&str]) -> Vec<Tool> {
    tools_full()
        .iter()
        .map(|tool| {
            let mut schema = tool.input_schema.as_ref().clone();
            if let Some(props) = schema.get_mut("properties").and_then(|v| v.as_object_mut()) {
                for f in fields {
                    props.remove(*f);
                }
            }
            if let Some(required) = schema.get_mut("required").and_then(|v| v.as_array_mut()) {
                required.retain(|v| !fields.iter().any(|f| v.as_str() == Some(f)));
            }
            let mut t = tool.clone();
            t.input_schema = Arc::new(schema);
            t
        })
        .collect()
}

fn tools_locked() -> &'static Vec<Tool> {
    TOOLS_LOCKED.get_or_init(|| strip_fields(&["project", "role"]))
}

fn tools_role_only() -> &'static Vec<Tool> {
    TOOLS_ROLE_ONLY.get_or_init(|| strip_fields(&["role"]))
}

use crate::config::Config;
use crate::mcp::knowledge::KnowledgeProvider;
use crate::mcp::memory::MemoryProvider;

/// Delegates to octolib::utils::path_to_id — single canonical implementation.
fn derive_project_id(path: &std::path::Path) -> String {
    octolib::utils::path_to_id(path)
}

/// Extract org/repo (lowercased) from a git remote URL via octolib::utils.
fn org_repo_from_url(url: &str) -> String {
    octolib::utils::org_repo_from_url(url)
}

/// Scan `root` for git repos: root itself, then immediate subdirectories.
/// Returns list of (org/repo label, hex project_id) for every git repo found.
fn discover_projects(root: &std::path::Path) -> Vec<(String, String)> {
    let mut found = Vec::new();

    let mut check = |path: &std::path::Path| {
        if path.join(".git").exists() {
            let id = derive_project_id(path);
            let label = std::process::Command::new("git")
                .args(["remote", "get-url", "origin"])
                .current_dir(path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| org_repo_from_url(&s))
                .unwrap_or_else(|| path.to_string_lossy().to_lowercase());
            found.push((label, id));
        }
    };

    check(root);

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                check(&path);
            }
        }
    }

    found
}

/// Build the instructions string, optionally including available project hints.
fn build_instructions(projects: &[(String, String)]) -> String {
    let base = "This server provides memory tools for storing and retrieving AI context. \
                Use 'memorize' to store information (supports 'related_to' for inline relationships), \
                'remember' for semantic search, 'forget' to delete memories, \
                and 'knowledge' to search/index/read/match indexed content. \
                The 'knowledge' tool's 'source' parameter is always a SINGLE FILE or URL — never a directory.";

    if projects.is_empty() {
        return base.to_string();
    }

    let mut hint =
        String::from("\n\nAvailable projects (pass the hex ID as the 'project' parameter):");
    for (label, id) in projects {
        hint.push_str(&format!("\n  {}: {}", label, id));
    }
    format!("{}{}", base, hint)
}

/// Session state for project/role locking from MCP capabilities
#[derive(Clone, Debug)]
pub struct SessionState {
    pub project: Option<String>,
    pub role: Option<String>,
    pub session_id: String,
    /// Role is locked (and stripped from schema) when role is present in handshake.
    pub role_locked: bool,
    /// Project is locked (and stripped from schema) when git=true OR no local repos.
    pub project_locked: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            project: None,
            role: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            role_locked: false,
            project_locked: false,
        }
    }
}

/// MCP Server using rmcp SDK
#[derive(Clone)]
pub struct McpServer {
    config: Config,
    working_directory: std::path::PathBuf,
    memory: Arc<Mutex<Option<MemoryProvider>>>,
    knowledge: Arc<Mutex<Option<KnowledgeProvider>>>,
    session: Arc<Mutex<SessionState>>,
    instructions: String,
    /// True when octobrain's working directory contains at least one git repo.
    has_local_projects: bool,
}

impl McpServer {
    pub fn new(config: Config, working_directory: std::path::PathBuf) -> Self {
        let projects = discover_projects(&working_directory);
        let has_local_projects = !projects.is_empty();
        let instructions = build_instructions(&projects);
        Self {
            config,
            working_directory,
            memory: Arc::new(Mutex::new(None)),
            knowledge: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(SessionState::default())),
            instructions,
            has_local_projects,
        }
    }

    /// Get memory provider.
    /// - Locked (handshake received): cached, project/role fixed from session state.
    /// - Unlocked (no handshake): fresh per call, project/role from caller args.
    async fn get_memory_provider(
        &self,
        project: Option<String>,
        role: Option<String>,
    ) -> Result<MemoryProvider, McpError> {
        let session = self.session.lock().await.clone();

        if session.role_locked || session.project_locked {
            // Double-checked lock: cheap path first
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
            let provider = MemoryProvider::new(
                &self.config,
                self.working_directory.clone(),
                session.project,
                session.role,
            )
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Failed to initialize memory: {}", e), None)
            })?;
            *guard = Some(provider.clone());
            Ok(provider)
        } else {
            // No handshake — honour per-call project/role from args
            MemoryProvider::new(&self.config, self.working_directory.clone(), project, role)
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("Failed to initialize memory: {}", e), None)
                })
        }
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

    /// Run server using HTTP transport (streamable HTTP for MCP 2025-03-26)
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

/// Convert a provider-layer `McpError` (crate::mcp::types) into the rmcp SDK error type.
fn to_rmcp_error(e: crate::mcp::types::McpError) -> McpError {
    McpError::internal_error(
        e.message,
        Some(serde_json::to_value(e.operation).unwrap_or_default()),
    )
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
    Validation,
    Research,
    Workflow,
    Requirement,
    Design,
    Integration,
    Communication,
    Process,
    Insight,
    /// Catch-all for unrecognized types — maps to Insight internally
    #[serde(other)]
    Other,
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
    /// This memory contributes to / advances a Goal memory.
    /// Use when memorizing context tied to a goal — `consolidate(goal_id)` later
    /// folds all Achieves sources into a single consolidated parent.
    Achieves,
}

/// A relationship to create alongside a `memorize` call.
/// Subsumes what the standalone `relate` tool used to do; one MCP round-trip
/// stores the memory AND links it to existing memories.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelationshipSpec {
    /// ID of the target memory to link to
    pub target_id: String,
    /// Relationship type
    pub relationship_type: RelationshipKind,
    /// Relationship strength 0.0-1.0 (default 0.8 if omitted)
    #[schemars(range(min = 0.0, max = 1.0))]
    pub strength: Option<f32>,
    /// Optional human description of why these memories are related
    #[schemars(length(max = 200))]
    pub description: Option<String>,
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
    /// Optional: create typed relationships from this new memory to existing
    /// memories in the same call. Subsumes the standalone relate tool.
    /// Most common use: contributing toward a Goal via
    /// `{ target_id: goal_id, relationship_type: "achieves" }`, then later
    /// closing it with `consolidate(goal_id)`.
    #[schemars(length(max = 20))]
    pub related_to: Option<Vec<RelationshipSpec>>,
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

/// Command for the knowledge tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeAction {
    /// Semantic search across indexed knowledge
    Search,
    /// Store raw text content under a unique key (session-scoped)
    Store,
    /// Delete stored content by key
    Delete,
    /// Read full content of a URL or local file (fallback when search is insufficient)
    Read,
    /// Search indexed content by regex pattern (like grep)
    Match,
}

/// Knowledge tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeParams {
    /// Command to execute
    pub command: KnowledgeAction,
    /// [search] What to search for, in natural language (required for search)
    #[schemars(length(min = 3, max = 500))]
    pub query: Option<String>,
    /// [search] Source filter — a SINGLE URL or local FILE path to auto-index and search within. MUST point to one specific file (e.g. /path/to/notes.md, https://example.com/page) — directories are NOT supported and will be rejected. Supports http/https URLs, file:///path, or /absolute/path. File types: .html, .txt, .md, .pdf, .docx. Omit to search across ALL previously indexed sources.
    /// [read] A SINGLE URL or local FILE path to read full content from. MUST point to one specific file — directories are NOT supported. Supports http/https URLs, file:///path, or /absolute/path. File types: .html, .txt, .md, .pdf, .docx.
    /// [match] Source filter — a SINGLE URL or local FILE path. MUST point to one specific file — directories are NOT supported. Omit to match across ALL indexed sources.
    pub source: Option<String>,
    /// [store/delete] Unique identifier key for the content. Error if key already exists on store — delete first to replace.
    pub key: Option<String>,
    /// [store] Raw text content to store and index (required for store)
    pub content: Option<String>,
    /// [match] Regex pattern to search for in indexed content (e.g., "error_code" or "timeout|retry")
    #[schemars(length(min = 1))]
    pub pattern: Option<String>,
}

// ============================================================================
// Tool implementations using rmcp macros
// ============================================================================

#[tool_router]
impl McpServer {
    #[tool(
        name = "memorize",
        description = "Store information, insights, or context in memory. Call remember first to avoid duplicates. Set source='user_confirmed' for user-stated facts (importance 0.8-1.0), 'agent_inferred' for AI conclusions (0.3-0.6). Skip transient state or things easily re-derived.\n\nUse related_to[] to link the new memory to existing ones in the same call. Relationship types: related_to, depends_on, supersedes, similar, conflicts, implements, extends, achieves, closes.\n\nGoal workflow:\n1. memorize a 'goal' type memory for the task — captures intent\n2. For each contributing memory: memorize with related_to=[{target_id: goal_id, relationship_type: 'achieves'}]\n3. When the task closes: memorize the completion / lesson-learned note with related_to=[{target_id: goal_id, relationship_type: 'closes'}]. This triggers automatic consolidation — your closing memo becomes the consolidated parent, all Achieves sources transition to Consolidated state with dampened importance (still queryable for audit). Importance of the closing memo is bumped to max(sources) * 1.1. No separate consolidate call needed."
    )]
    async fn memorize(
        &self,
        Parameters(params): Parameters<MemorizeParams>,
    ) -> Result<String, McpError> {
        let provider = self
            .get_memory_provider(params.project.clone(), params.role.clone())
            .await?;
        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;
        provider
            .execute_memorize(&args)
            .await
            .map_err(to_rmcp_error)
    }

    #[tool(
        name = "remember",
        description = "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. Prefer 2-5 related query terms for broader coverage. Results show [CONFIRMED]/[INFERRED] trust labels."
    )]
    async fn remember(
        &self,
        Parameters(params): Parameters<RememberParams>,
    ) -> Result<String, McpError> {
        let provider = self
            .get_memory_provider(params.project.clone(), params.role.clone())
            .await?;
        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;
        provider
            .execute_remember(&args)
            .await
            .map_err(to_rmcp_error)
    }

    #[tool(
        name = "forget",
        description = "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal. Don't forget memories just because they're old — importance decay handles that. Only delete when information is wrong or superseded."
    )]
    async fn forget(
        &self,
        Parameters(params): Parameters<ForgetParams>,
    ) -> Result<String, McpError> {
        let provider = self
            .get_memory_provider(params.project.clone(), params.role.clone())
            .await?;
        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;
        provider.execute_forget(&args).await.map_err(to_rmcp_error)
    }

    #[tool(
        name = "knowledge",
        description = "Knowledge base with five commands. The 'source' parameter (when used) ALWAYS refers to a SINGLE FILE or URL — never a directory; passing a directory path is an error. 'search': semantic search across indexed content — provide source (single URL or file) to auto-index on-the-fly, omit to search all indexed sources. 'store': save raw text under a unique key (session-scoped, auto-cleaned) — error if key exists, delete first to replace. 'delete': remove stored content by key. 'read': fetch and return the FULL text content of a single URL or file — use ONLY as a last resort when search results are insufficient; prefer 'search' for targeted retrieval. 'match': search indexed content by regex pattern (like grep) — returns matching lines only; prefer 'search' for semantic queries, use 'match' for exact string/regex patterns. Supported file types: .html, .txt, .md, .pdf, .docx."
    )]
    async fn knowledge(
        &self,
        Parameters(params): Parameters<KnowledgeParams>,
    ) -> Result<String, McpError> {
        let provider = self.get_or_init_knowledge().await?;
        let session = self.session.lock().await;
        let session_id = session.session_id.clone();
        drop(session);

        match params.command {
            KnowledgeAction::Search => {
                provider
                    .execute_search(
                        params.query.as_deref(),
                        params.source.as_deref(),
                        &session_id,
                    )
                    .await
            }
            KnowledgeAction::Store => {
                provider
                    .execute_store(
                        params.key.as_deref(),
                        params.content.as_deref(),
                        &session_id,
                    )
                    .await
            }
            KnowledgeAction::Delete => {
                provider
                    .execute_delete(params.key.as_deref(), &session_id)
                    .await
            }
            KnowledgeAction::Read => provider.execute_read(params.source.as_deref()).await,
            KnowledgeAction::Match => {
                provider
                    .execute_match(
                        params.pattern.as_deref(),
                        params.source.as_deref(),
                        &session_id,
                    )
                    .await
            }
        }
        .map_err(to_rmcp_error)
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
            .with_instructions(self.instructions.clone())
    }

    /// Return tool list with project/role stripped from schemas when session context is known
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let session = self.session.lock().await;
        let tools = if session.role_locked && session.project_locked {
            tools_locked().clone() // strip project + role
        } else if session.role_locked {
            tools_role_only().clone() // strip role only, project stays visible
        } else {
            tools_full().clone()
        };
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
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
                let session_id = session_obj
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let git = session_obj
                    .get("git")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut session = self.session.lock().await;
                let should_lock_project = git || !self.has_local_projects;
                session.project = if should_lock_project { project } else { None };
                session.role = role;
                if let Some(sid) = session_id {
                    session.session_id = sid;
                }
                // Always lock (handshake received) — strips role from schema.
                // project_locked strips project from schema too, only when meaningful.
                session.role_locked = session.role.is_some();
                session.project_locked = should_lock_project;

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
