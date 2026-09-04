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
//! Provides MCP 2026-07-28 protocol compliance with stdio and HTTP transports

use anyhow::Result;
use rmcp::{
    handler::server::{wrapper::Parameters, ServerHandler},
    model::{
        Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// How long stdio shutdown waits for an in-flight maintenance pass before giving up.
/// Long enough for a normal incremental compaction, short enough that a wedged pass
/// can't strand the process after the client is gone.
const MAINTENANCE_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(300);

/// Tools with scope+role stripped — built once.
static TOOLS_LOCKED: OnceLock<Vec<Tool>> = OnceLock::new();
/// Tools with only role stripped — built once.
static TOOLS_ROLE_ONLY: OnceLock<Vec<Tool>> = OnceLock::new();
/// Full tools list — built once.
static TOOLS_FULL: OnceLock<Vec<Tool>> = OnceLock::new();
/// Full tools without global flag (unlocked sessions don't need it — use scope="" directly).
static TOOLS_FULL_NO_GLOBAL: OnceLock<Vec<Tool>> = OnceLock::new();

fn tools_full() -> &'static Vec<Tool> {
    TOOLS_FULL.get_or_init(|| {
        McpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|mut tool| {
                let mut schema = tool.input_schema.as_ref().clone();
                inline_schema_refs(&mut schema);
                tool.input_schema = Arc::new(schema);
                tool
            })
            .collect()
    })
}

/// Inline every `$ref` into its `$defs` target, drop `$defs`/`$schema`, and
/// strip the `null` branch from nullable types — producing flat, scalar-typed
/// self-contained tool schemas.
///
/// Two backend quirks make this the portable shape for tool calling:
/// - `$ref` indirection: some backends (e.g. Together's
///   `thinkingmachines/Inkling`) silently return an EMPTY generation.
/// - nullable types: schemars emits `"type": ["number", "null"]` and
///   `anyOf: [{...}, {"type": "null"}]` for `Option<T>` fields. Alibaba Model
///   Studio's compatible-mode endpoint resolves an argument's type from a
///   scalar `"type"` only; on the array form it falls back to string and
///   constrains the model to emit `"0.6"` instead of `0.6`, which then fails
///   this server's own parameter deserialization. Verified live across
///   qwen3.8-max, qwen3.7-max/plus and qwen3.6-flash.
///
/// Dropping the null branch is lossless: optionality is carried by `required`.
fn inline_schema_refs(schema: &mut serde_json::Map<String, serde_json::Value>) {
    let defs = match schema.remove("$defs") {
        Some(serde_json::Value::Object(defs)) => defs,
        _ => serde_json::Map::new(),
    };
    schema.remove("$schema");
    for value in schema.values_mut() {
        resolve_refs(value, &defs);
    }
}

fn resolve_refs(value: &mut serde_json::Value, defs: &serde_json::Map<String, serde_json::Value>) {
    match value {
        serde_json::Value::Object(obj) => {
            if let Some(reference) = obj.remove("$ref") {
                let name = reference
                    .as_str()
                    .and_then(|r| r.rsplit('/').next())
                    .unwrap_or_default();
                let mut target = defs
                    .get(name)
                    .unwrap_or_else(|| panic!("unresolved $ref to '{}' in tool schema", name))
                    .clone();
                resolve_refs(&mut target, defs);
                if let serde_json::Value::Object(target) = target {
                    for (k, v) in target {
                        // Sibling keys on the $ref site (e.g. a field-level
                        // description) take precedence over the def's own.
                        obj.entry(k).or_insert(v);
                    }
                }
            }
            for nested in obj.values_mut() {
                resolve_refs(nested, defs);
            }
            strip_null_variants(obj);
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                resolve_refs(item, defs);
            }
        }
        _ => {}
    }
}

/// Collapse `"type": [T, "null"]` to `T` and `anyOf: [X, {"type": "null"}]` to
/// `X`, merging the surviving variant into the parent so field-level keys
/// (description) win over the inlined ones.
fn strip_null_variants(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let collapsed_type = obj
        .get_mut("type")
        .and_then(|t| t.as_array_mut())
        .and_then(|types| {
            types.retain(|t| t.as_str() != Some("null"));
            (types.len() == 1).then(|| types[0].clone())
        });
    if let Some(single) = collapsed_type {
        obj.insert("type".to_string(), single);
    }

    for key in ["anyOf", "oneOf"] {
        let only = obj
            .get_mut(key)
            .and_then(|v| v.as_array_mut())
            .and_then(|variants| {
                variants.retain(|v| v.get("type").and_then(|t| t.as_str()) != Some("null"));
                (variants.len() == 1).then(|| variants[0].clone())
            });
        if let Some(serde_json::Value::Object(only)) = only {
            obj.remove(key);
            for (k, v) in only {
                obj.entry(k).or_insert(v);
            }
        }
    }
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

/// Unlocked full schema: strip `global` (irrelevant — AI can pass scope="" directly).
fn tools_full_no_global() -> &'static Vec<Tool> {
    TOOLS_FULL_NO_GLOBAL.get_or_init(|| strip_fields(&["global"]))
}

/// Scope locked: strip `scope` + `role` + keep `global` so AI can opt into global scope.
fn tools_locked() -> &'static Vec<Tool> {
    TOOLS_LOCKED.get_or_init(|| strip_fields(&["scope", "role"]))
}

/// Role locked only: strip `role` + `global` (scope still free, no lock).
fn tools_role_only() -> &'static Vec<Tool> {
    TOOLS_ROLE_ONLY.get_or_init(|| strip_fields(&["role", "global"]))
}

use crate::config::Config;
use crate::mcp::knowledge::KnowledgeProvider;
use crate::mcp::memory::MemoryProvider;

/// Scan `root` for git repos: root itself, then immediate subdirectories.
/// Returns list of human-readable scope strings for every git repo found.
fn discover_scopes(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();

    let mut check = |path: &std::path::Path| {
        if path.join(".git").exists() {
            found.push(crate::storage::derive_scope(path));
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

/// Like `discover_scopes`, but returns (repo_path, scope) pairs — used by box sync
/// to locate each project's `.box/` directory and bind it to the project scope.
fn discover_projects(root: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut found = Vec::new();

    let mut check = |path: &std::path::Path| {
        if path.join(".git").exists() {
            found.push((path.to_path_buf(), crate::storage::derive_scope(path)));
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

/// Build the instructions string, optionally including available scope hints.
fn build_instructions(scopes: &[String]) -> String {
    let base = "This server provides memory tools for storing and retrieving AI context. \
                Use 'memorize' to store information (supports 'related_to' for inline relationships), \
                'remember' for semantic search, 'forget' to delete memories, \
                and 'knowledge' to search/index/read/match indexed content. \
                The 'knowledge' tool's 'source' parameter is always a SINGLE FILE or URL — never a directory.";

    if scopes.is_empty() {
        return base.to_string();
    }

    let mut hint = String::from("\n\nAvailable project scopes (pass as the 'scope' parameter):");
    for scope in scopes {
        hint.push_str(&format!("\n  {}", scope));
    }
    format!("{}{}", base, hint)
}

/// Session state for scope/role locking from MCP capabilities
#[derive(Clone, Debug)]
pub struct SessionState {
    pub scope: Option<String>,
    pub role: Option<String>,
    pub session_id: String,
    /// Role is locked (and stripped from schema) when role is present in handshake.
    pub role_locked: bool,
    /// Scope is locked (and stripped from schema) when git=true OR no local repos.
    pub scope_locked: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            scope: None,
            role: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            role_locked: false,
            scope_locked: false,
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
    /// Whether the session context has been applied from client capabilities.
    /// Applied once on the first request so a later state change is not
    /// overwritten; also single-flights the background box sync.
    session_applied: Arc<AtomicBool>,
}

impl McpServer {
    pub fn new(config: Config, working_directory: std::path::PathBuf) -> Self {
        let scopes = discover_scopes(&working_directory);
        let has_local_projects = !scopes.is_empty();
        let instructions = build_instructions(&scopes);
        Self {
            config,
            working_directory,
            memory: Arc::new(Mutex::new(None)),
            knowledge: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(SessionState::default())),
            instructions,
            has_local_projects,
            session_applied: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Apply the octomind session context from client capabilities
    /// (`experimental.session`) on the first request.
    ///
    /// Works for both protocol eras: modern clients (2026-07-28) carry
    /// capabilities in every request's `_meta`, legacy clients set them during
    /// the `initialize` handshake — `RequestContext::client_capabilities()`
    /// resolves both.
    async fn ensure_session_context(&self, context: &RequestContext<RoleServer>) {
        if self.session_applied.swap(true, Ordering::SeqCst) {
            return;
        }

        if let Some(capabilities) = context.client_capabilities() {
            if let Some(experimental) = &capabilities.experimental {
                if let Some(session_obj) = experimental.get("session") {
                    let scope = session_obj
                        .get("scope")
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
                    let should_lock_scope = git || !self.has_local_projects;
                    session.scope = if should_lock_scope { scope } else { None };
                    session.role = role;
                    if let Some(sid) = session_id {
                        session.session_id = sid;
                    }
                    // Always lock (session received) — strips role from schema.
                    // scope_locked strips scope from schema too, only when meaningful.
                    session.role_locked = session.role.is_some();
                    session.scope_locked = should_lock_scope;

                    debug!(
                        "Session locked: scope={:?}, role={:?}",
                        session.scope, session.role
                    );
                }
            }
        }

        // Kick off non-blocking box discovery/sync now that any session scope is
        // established. Safe without a session too: it locates project .box/ dirs
        // from the working directory and refreshes subscribed boxes.
        let server = self.clone();
        tokio::spawn(async move {
            server.run_box_sync_background().await;
        });
    }

    /// Background, non-blocking box discovery + sync. Logs and swallows errors so a
    /// missing embedding provider or offline remote never disrupts the session.
    async fn run_box_sync_background(&self) {
        let provider = match self.get_or_init_knowledge().await {
            Ok(p) => p,
            Err(e) => {
                debug!("Box sync skipped (knowledge init failed): {:?}", e);
                return;
            }
        };
        let projects = discover_projects(&self.working_directory);
        if let Err(e) = provider.sync_boxes(&projects).await {
            debug!("Box sync failed: {:?}", e);
        }
    }

    /// Get memory provider.
    /// - Locked (handshake received): cached, scope/role fixed from session state.
    /// - Unlocked (no handshake): fresh per call, scope/role from caller args.
    async fn get_memory_provider(
        &self,
        scope: Option<String>,
        role: Option<String>,
    ) -> Result<MemoryProvider, McpError> {
        let session = self.session.lock().await.clone();

        if session.role_locked || session.scope_locked {
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
                session.scope,
                session.role,
            )
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Failed to initialize memory: {}", e), None)
            })?;
            *guard = Some(provider.clone());
            Ok(provider)
        } else {
            // No handshake — honour per-call scope/role from args
            MemoryProvider::new(&self.config, self.working_directory.clone(), scope, role)
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
        let memory = self.memory.clone();

        self.serve(transport)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize MCP server: {}", e))?
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP server task failed: {}", e))?;

        // Returning here drops the runtime and every task on it. A LanceDB compaction
        // pass takes far longer than the client's disconnect, so without this wait it is
        // killed part-way every session and the table never gets compacted. Capped so a
        // stuck pass can't keep the process alive indefinitely.
        let provider = memory.lock().await.clone();
        if let Some(provider) = provider {
            if tokio::time::timeout(
                MAINTENANCE_SHUTDOWN_GRACE,
                provider.drain_pending_maintenance(),
            )
            .await
            .is_err()
            {
                warn!("maintenance still running at shutdown; abandoning it");
            }
        }

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

// NOTE: variants in these enums must NOT carry doc comments — schemars turns a
// documented variant into a `oneOf`-of-`const` schema, and some inference
// backends (e.g. Together's Inkling endpoint) silently return an empty
// generation when a `$ref` with sibling keys points at a `oneOf`/`anyOf` def.
// Plain variants collapse into a portable `"enum": [...]`; describe the values
// in the type-level or field-level doc instead (same pattern as octofs).

/// Memory category for organization and filtering.
/// 'other' is a catch-all for unrecognized types — maps to Insight internally.
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
    #[serde(other)]
    Other,
}

/// Trust tier for memory source attribution: 'user_confirmed' — user explicitly
/// stated or approved this fact; 'agent_inferred' — AI-inferred conclusion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceTrust {
    UserConfirmed,
    AgentInferred,
}

/// Relationship type between memories: related_to (general association),
/// depends_on (A needs B), supersedes (A replaces B), similar (near-duplicate),
/// conflicts (contradicts), implements (concrete implementation of abstract
/// concept), extends (builds on top of), achieves (this memory contributes to /
/// advances a Goal memory — `consolidate(goal_id)` later folds all Achieves
/// sources into a single consolidated parent).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    RelatedTo,
    DependsOn,
    Supersedes,
    Similar,
    Conflicts,
    Implements,
    Extends,
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
    Single(String),
    Multiple(Vec<String>),
}

/// Inline `anyOf` schema for [`QueryInput`]. Hand-written (octofs-style) so the
/// field gets an inline composition instead of a `$ref` into `$defs` — a `$ref`
/// with a sibling `description` pointing at an `anyOf` def breaks some
/// inference backends (empty generation). `anyOf` over `oneOf` for wider
/// cross-stack support.
fn query_input_schema(_gen: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "anyOf": [
            {
                "type": "string",
                "description": "Single semantic search query"
            },
            {
                "type": "array",
                "items": { "type": "string" },
                "description": "2-5 related terms for comprehensive coverage — preferred over single query"
            }
        ]
    }))
    .expect("static schema is valid JSON")
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
    /// Scope string to store this memory under (e.g. 'github.com/org/repo'). Defaults to auto-detected scope from cwd.
    pub scope: Option<String>,
    /// When true, store this memory in the global scope (shared across all projects) regardless of the locked session scope.
    pub global: Option<bool>,
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
    #[schemars(schema_with = "query_input_schema")]
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
    /// Only return memories created on/after this time (ISO-8601, e.g. "2026-06-01" or
    /// "2026-06-01T00:00:00Z"). Use for temporal questions like "what did I decide last week".
    pub created_after: Option<String>,
    /// Only return memories created on/before this time (ISO-8601). Pair with created_after
    /// to scope a window (e.g. a specific month).
    pub created_before: Option<String>,
    /// Filter by scope. If omitted, returns memories from all scopes (global + current).
    pub scope: Option<String>,
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
    /// Scope filter
    pub scope: Option<String>,
    /// Role filter
    pub role: Option<String>,
}

/// Command for the knowledge tool: search (semantic search across indexed
/// knowledge), store (store raw text content under a unique key,
/// session-scoped), delete (delete stored content by key), read (read full
/// content of a URL or local file — fallback when search is insufficient),
/// match (search indexed content by regex pattern, like grep).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeAction {
    Search,
    Store,
    Delete,
    Read,
    Match,
}

/// Knowledge tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeParams {
    /// Command to execute: search, store, delete, read, match
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
        description = "Store information, insights, or context in memory. Call remember first to avoid duplicates. Set source='user_confirmed' for user-stated facts (importance 0.8-1.0), 'agent_inferred' for AI conclusions (0.3-0.6). Skip transient state or things easily re-derived.\n\nUse related_to[] to link the new memory to existing ones in the same call. Relationship types: related_to, depends_on, supersedes, similar, conflicts, implements, extends, achieves, closes.\n\nKnowledge updates: when a new fact replaces or corrects an existing memory (a value changed, a decision was reversed), memorize the new fact with related_to=[{target_id: <old_id>, relationship_type: 'supersedes'}]. Retrieval then ranks the current fact above the outdated one, which stays queryable for history. remember first to find the old_id.\n\nGoal workflow:\n1. memorize a 'goal' type memory for the task — captures intent\n2. For each contributing memory: memorize with related_to=[{target_id: goal_id, relationship_type: 'achieves'}]\n3. When the task closes: memorize the completion / lesson-learned note with related_to=[{target_id: goal_id, relationship_type: 'closes'}]. This triggers automatic consolidation — your closing memo becomes the consolidated parent, all Achieves sources transition to Consolidated state with dampened importance (still queryable for audit). Importance of the closing memo is bumped to max(sources) * 1.1. No separate consolidate call needed.\n\nglobal=true (only available when session scope is locked): stores the memory in the shared global scope instead of the locked project scope. Use this for cross-project facts — user preferences, personal habits, universal conventions, tool choices — things that apply regardless of which project is active. Do NOT use global for project-specific knowledge.",
        annotations(
            title = "Memorize",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn memorize(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<MemorizeParams>,
    ) -> Result<String, McpError> {
        self.ensure_session_context(&context).await;
        // global=true only meaningful when scope is locked; overrides to global scope ("")
        let session_scope_locked = self.session.lock().await.scope_locked;
        let effective_scope = if session_scope_locked && params.global == Some(true) {
            Some(String::new())
        } else {
            params.scope.clone()
        };
        let provider = self
            .get_memory_provider(effective_scope, params.role.clone())
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
        description = "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. Prefer 2-5 related query terms for broader coverage. Results show [CONFIRMED]/[INFERRED] trust labels. For temporal questions (\"last week\", \"in May\"), set created_after/created_before (ISO-8601) to scope the time window — you know today's date, so compute the bounds.",
        annotations(title = "Remember", read_only_hint = true, open_world_hint = false)
    )]
    async fn remember(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<RememberParams>,
    ) -> Result<String, McpError> {
        self.ensure_session_context(&context).await;
        let provider = self
            .get_memory_provider(params.scope.clone(), params.role.clone())
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
        description = "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal. Don't forget memories just because they're old — importance decay handles that. Only delete when information is wrong or superseded.",
        annotations(
            title = "Forget",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn forget(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ForgetParams>,
    ) -> Result<String, McpError> {
        self.ensure_session_context(&context).await;
        let provider = self
            .get_memory_provider(params.scope.clone(), params.role.clone())
            .await?;
        let args = serde_json::to_value(&params).map_err(|e| {
            McpError::internal_error(format!("Failed to serialize params: {}", e), None)
        })?;
        provider.execute_forget(&args).await.map_err(to_rmcp_error)
    }

    #[tool(
        name = "knowledge",
        description = "Knowledge base with five commands. The 'source' parameter (when used) ALWAYS refers to a SINGLE FILE or URL — never a directory; passing a directory path is an error. URLs are fetched anonymously — public documentation and information pages only, never API endpoints that need credentials (GitHub Actions logs, private repos, etc. return 403; use the platform's own CLI for those). 'search': semantic search across indexed content — provide source (single URL or file) to auto-index on-the-fly, omit to search all indexed sources. 'store': save raw text under a unique key (session-scoped, auto-cleaned) — error if key exists, delete first to replace. 'delete': remove stored content by key. 'read': fetch and return the FULL text content of a single URL or file — use ONLY as a last resort when search results are insufficient; prefer 'search' for targeted retrieval. 'match': search indexed content by regex pattern (like grep) — returns matching lines only; prefer 'search' for semantic queries, use 'match' for exact string/regex patterns. Supported file types: .html, .txt, .md, .pdf, .docx.",
        // Mixed commands: `delete` mutates stored content and URL fetch
        // reaches the open world — spec defaults kept deliberately.
        annotations(title = "Knowledge", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
    async fn knowledge(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<KnowledgeParams>,
    ) -> Result<String, McpError> {
        self.ensure_session_context(&context).await;
        let provider = self.get_or_init_knowledge().await?;
        let session = self.session.lock().await;
        let session_id = session.session_id.clone();
        let active_scope = session.scope.clone();
        drop(session);

        match params.command {
            KnowledgeAction::Search => {
                provider
                    .execute_search(
                        params.query.as_deref(),
                        params.source.as_deref(),
                        &session_id,
                        active_scope.as_deref(),
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
                        active_scope.as_deref(),
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
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .with_server_info(
                Implementation::new("octobrain", env!("CARGO_PKG_VERSION"))
                    .with_title("Octobrain Memory Server")
                    .with_description(
                        "Standalone memory management system for AI context and conversation state",
                    ),
            )
            .with_instructions(self.instructions.clone())
    }

    /// Return tool list with scope/role stripped from schemas when session context is known
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.ensure_session_context(&context).await;
        let session = self.session.lock().await;
        let tools = if session.role_locked && session.scope_locked {
            tools_locked().clone() // strip scope + role
        } else if session.role_locked {
            tools_role_only().clone() // strip role only, scope stays visible
        } else {
            tools_full_no_global().clone()
        };
        Ok(ListToolsResult::with_all_items(tools))
    }

    // The default `initialize` (legacy clients) and `discover` (2026-07-28
    // clients) implementations handle peer info and version negotiation; the
    // session scope/role is applied per-request in `ensure_session_context`,
    // which covers both eras.
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// Every advertised tool schema must carry a scalar `"type"` and no
    /// null-only `anyOf` branch. Qwen models (verified on qwen3.8-max /
    /// qwen3.7-max / qwen3.6-flash via Alibaba Model Studio) fail to resolve a
    /// non-scalar type and emit that argument as a string — `"0.6"` instead of
    /// `0.6` — which this server then rejects while deserializing parameters.
    #[test]
    fn tool_schemas_carry_scalar_types() {
        fn check(value: &serde_json::Value, path: &str) {
            match value {
                serde_json::Value::Object(obj) => {
                    assert!(
                        !obj.get("type").is_some_and(|t| t.is_array()),
                        "non-scalar type at {}: {}",
                        path,
                        value
                    );
                    for key in ["anyOf", "oneOf"] {
                        if let Some(serde_json::Value::Array(variants)) = obj.get(key) {
                            assert!(
                                !variants
                                    .iter()
                                    .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("null")),
                                "null variant in {} at {}",
                                key,
                                path
                            );
                        }
                    }
                    for (k, v) in obj {
                        check(v, &format!("{}.{}", path, k));
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        check(item, path);
                    }
                }
                _ => {}
            }
        }

        for tool in tools_full() {
            let schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());
            check(&schema, &tool.name);
        }
    }

    /// Annotations must be present on every tool and survive every session
    /// variant — `strip_fields` rewrites only `input_schema`.
    #[test]
    fn tool_annotations_survive_strip_variants() {
        // (name, read_only, destructive, idempotent, open_world)
        type AnnotationRow = (
            &'static str,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
        );
        let expected: &[AnnotationRow] = &[
            (
                "memorize",
                Some(false),
                Some(false),
                Some(false),
                Some(false),
            ),
            ("remember", Some(true), None, None, Some(false)),
            ("forget", Some(false), Some(true), Some(true), Some(false)),
            (
                "knowledge",
                Some(false),
                Some(true),
                Some(false),
                Some(true),
            ),
        ];
        for variant in [
            tools_full(),
            tools_full_no_global(),
            tools_locked(),
            tools_role_only(),
        ] {
            for (name, read_only, destructive, idempotent, open_world) in expected {
                let tool = variant
                    .iter()
                    .find(|t| t.name.as_ref() == *name)
                    .unwrap_or_else(|| panic!("{name} missing from variant"));
                let a = tool
                    .annotations
                    .as_ref()
                    .unwrap_or_else(|| panic!("{name} has no annotations"));
                assert!(
                    !a.title.as_deref().unwrap_or_default().is_empty(),
                    "{name} title"
                );
                assert_eq!(a.read_only_hint, *read_only, "{name} read_only_hint");
                assert_eq!(a.destructive_hint, *destructive, "{name} destructive_hint");
                assert_eq!(a.idempotent_hint, *idempotent, "{name} idempotent_hint");
                assert_eq!(a.open_world_hint, *open_world, "{name} open_world_hint");
            }
        }
    }
}
