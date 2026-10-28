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
		CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
		ServerConfig, Tool,
	},
	schemars::JsonSchema,
	service::RequestContext,
	tool, tool_handler, tool_router,
	transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpService},
	ErrorData as McpError, RoleServer, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
/// Tools with only scope stripped — built once.
static TOOLS_SCOPE_ONLY: OnceLock<Vec<Tool>> = OnceLock::new();
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
	let collapsed_type = obj.get_mut("type").and_then(|t| t.as_array_mut()).and_then(|types| {
		types.retain(|t| t.as_str() != Some("null"));
		(types.len() == 1).then(|| types[0].clone())
	});
	if let Some(single) = collapsed_type {
		obj.insert("type".to_string(), single);
	}

	for key in ["anyOf", "oneOf"] {
		let only = obj.get_mut(key).and_then(|v| v.as_array_mut()).and_then(|variants| {
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

/// Scope and role locked: strip `scope` + `role` + keep `global` so AI can opt into global scope.
fn tools_locked() -> &'static Vec<Tool> {
	TOOLS_LOCKED.get_or_init(|| strip_fields(&["scope", "role"]))
}

/// Role locked only: strip `role` + `global` (scope still free, no lock).
fn tools_role_only() -> &'static Vec<Tool> {
	TOOLS_ROLE_ONLY.get_or_init(|| strip_fields(&["role", "global"]))
}

/// Scope locked only: strip `scope`, keep `role` + `global` so AI can opt into global scope.
fn tools_scope_only() -> &'static Vec<Tool> {
	TOOLS_SCOPE_ONLY.get_or_init(|| strip_fields(&["scope"]))
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
			found.push(octolib::utils::path_to_scope(path));
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
			found.push((path.to_path_buf(), octolib::utils::path_to_scope(path)));
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

/// `discover_projects` off the async runtime — it walks the directory synchronously.
async fn discover_session_projects(workdir: std::path::PathBuf) -> Result<Vec<(std::path::PathBuf, String)>, McpError> {
	tokio::task::spawn_blocking(move || discover_projects(&workdir))
		.await
		.map_err(|e| McpError::internal_error(format!("Failed to discover projects: {}", e), None))
}

/// Build the instructions string, optionally including available scope hints.
fn build_instructions(scopes: &[String]) -> String {
	let base = "Octobrain gives you durable memory and a searchable knowledge base. Choose where to keep something by what it is:\n\
		- 'memorize': one fact, decision, preference or lesson to recall in later sessions (short and atomic; link it via 'related_to'). 'remember' searches memories, 'forget' deletes them.\n\
		- Knowledge box: durable reference documents (rules, guides, decisions, specs) as Markdown files under '<repo>/.box/', versioned and shared with the repo. Write them with your file tools, then run knowledge 'sync'; knowledge 'search' and 'match' cover them.\n\
		- knowledge 'store': scratch text for this session only — never durable.\n\
		- knowledge 'search' with 'source': one external URL or local file, indexed on demand ('read' returns its full text instead).\n\
		The 'knowledge' tool's 'source' parameter is always a SINGLE FILE or URL — never a directory.";

	if scopes.is_empty() {
		return base.to_string();
	}

	let mut hint = String::from(
		"\n\nAvailable project scopes (the optional 'scope' parameter takes one of these, or '' for global):",
	);
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
	/// Scope is locked for a Git root or a working directory without local repos.
	pub scope_locked: bool,
	/// Unlocked session (its workdir holds repos): the scope used when a call names
	/// none — the workdir's own. `None` without session context.
	pub default_scope: Option<String>,
	/// Unlocked session: the repos discovered under its workdir, the other scopes a
	/// call may name.
	pub scope_choices: Vec<String>,
	/// Directory whose repos this session works in — where its project `.box/` dirs live.
	pub workdir: std::path::PathBuf,
}

impl Default for SessionState {
	fn default() -> Self {
		Self {
			scope: None,
			role: None,
			session_id: uuid::Uuid::new_v4().to_string(),
			role_locked: false,
			scope_locked: false,
			default_scope: None,
			scope_choices: Vec::new(),
			workdir: std::path::PathBuf::new(),
		}
	}
}

impl SessionState {
	/// Scopes an unlocked session may name: its own, the repos under its workdir,
	/// and the global scope `""`.
	fn scope_options(&self) -> Vec<String> {
		let mut options: Vec<String> = self.default_scope.iter().cloned().collect();
		for scope in &self.scope_choices {
			if !options.contains(scope) {
				options.push(scope.clone());
			}
		}
		options.push(String::new());
		options
	}

	/// The scope a memory call runs in. A session never runs unscoped (every
	/// project's memories): an omitted scope is its own, and a named one must be
	/// one of its options. Without session context the caller's scope is used as is.
	fn resolve_scope(&self, scope: Option<String>) -> Result<Option<String>, McpError> {
		if self.scope_locked {
			return Ok(self.scope.clone());
		}
		let Some(default_scope) = &self.default_scope else {
			return Ok(scope);
		};
		let Some(scope) = scope else {
			return Ok(Some(default_scope.clone()));
		};
		let options = self.scope_options();
		if options.contains(&scope) {
			return Ok(Some(scope));
		}
		Err(McpError::invalid_params(
			format!(
				"scope '{}' is not available in this session; use one of: {}",
				scope,
				options
					.iter()
					.map(|option| format!("'{}'", option))
					.collect::<Vec<_>>()
					.join(", ")
			),
			None,
		))
	}
}

// Octomind also sends `project` (`octolib::utils::path_to_id`); memory scopes are the
// readable `path_to_scope` of the same directory, so they are derived from `workdir`.
#[derive(Deserialize)]
struct SessionCapabilities {
	role: Option<String>,
	workdir: Option<String>,
	session_id: Option<String>,
	#[serde(default)]
	git: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct WorkdirScope {
	scope: String,
	/// Scopes of the repos at or directly under the workdir.
	projects: Vec<String>,
}

impl WorkdirScope {
	fn discover(workdir: &std::path::Path) -> Self {
		Self {
			scope: octolib::utils::path_to_scope(workdir),
			projects: discover_scopes(workdir),
		}
	}
}

type MemoryProviders = HashMap<(Option<String>, Option<String>), MemoryProvider>;

/// MCP Server using rmcp SDK
#[derive(Clone)]
pub struct McpServer {
	config: Config,
	working_directory: std::path::PathBuf,
	memory: Arc<Mutex<MemoryProviders>>,
	knowledge: Arc<Mutex<Option<KnowledgeProvider>>>,
	session: SessionState,
	instructions: String,
	workdir_scopes: Arc<Mutex<HashMap<std::path::PathBuf, WorkdirScope>>>,
	/// Workdirs whose boxes this server instance has synced — one background sync each, so
	/// every session workdir's repos get their `.box/` indexed, not just the server cwd.
	box_synced_workdirs: Arc<Mutex<HashSet<std::path::PathBuf>>>,
	/// Tests turn it off: a background sync would load a real embedding model.
	box_sync_enabled: Arc<AtomicBool>,
}

impl McpServer {
	pub fn new(config: Config, working_directory: std::path::PathBuf) -> Self {
		let scopes = discover_scopes(&working_directory);
		let workdir_scopes = HashMap::from([(
			working_directory.clone(),
			WorkdirScope {
				scope: octolib::utils::path_to_scope(&working_directory),
				projects: scopes.clone(),
			},
		)]);
		let instructions = build_instructions(&scopes);
		let session = SessionState {
			workdir: working_directory.clone(),
			..Default::default()
		};
		Self {
			config,
			working_directory,
			memory: Arc::new(Mutex::new(HashMap::new())),
			knowledge: Arc::new(Mutex::new(None)),
			session,
			instructions,
			workdir_scopes: Arc::new(Mutex::new(workdir_scopes)),
			box_synced_workdirs: Arc::new(Mutex::new(HashSet::new())),
			box_sync_enabled: Arc::new(AtomicBool::new(true)),
		}
	}

	/// Request metadata takes precedence over the legacy initialize capabilities.
	/// Keep the resolved context local: concurrent requests can belong to different octos.
	async fn ensure_session_context(&self, context: &RequestContext<RoleServer>) -> Result<SessionState, McpError> {
		let mut session = self.session.clone();
		if let Some(capabilities) = context.client_capabilities() {
			if let Some(experimental) = capabilities.experimental {
				if let Some(session_obj) = experimental.get("session") {
					let received: SessionCapabilities =
						serde_json::from_value(serde_json::Value::Object(session_obj.clone()))
							.map_err(|e| McpError::invalid_params(format!("Invalid session context: {}", e), None))?;
					// Octomind 0.57.1 and older send `"workdir": ""` when the session has no
					// anchor; like an absent workdir, that means this server's own directory.
					let workdir = received
						.workdir
						.filter(|path| !path.trim().is_empty())
						.map(std::path::PathBuf::from)
						.unwrap_or_else(|| self.working_directory.clone());
					session.workdir = workdir.clone();
					let workdir_scope = {
						let mut cache = self.workdir_scopes.lock().await;
						if let Some(workdir_scope) = cache.get(&workdir) {
							workdir_scope.clone()
						} else {
							let path = workdir.clone();
							let workdir_scope = tokio::task::spawn_blocking(move || WorkdirScope::discover(&path))
								.await
								.map_err(|e| {
									McpError::internal_error(format!("Failed to discover session scope: {}", e), None)
								})?;
							cache.insert(workdir, workdir_scope.clone());
							workdir_scope
						}
					};
					let should_lock_scope = received.git || workdir_scope.projects.is_empty();
					session.scope = should_lock_scope.then(|| workdir_scope.scope.clone());
					if !should_lock_scope {
						session.default_scope = Some(workdir_scope.scope);
						session.scope_choices = workdir_scope.projects;
					}
					session.role = received.role;
					if let Some(session_id) = received.session_id {
						session.session_id = session_id;
					}
					session.role_locked = session.role.is_some();
					session.scope_locked = should_lock_scope;
				}
			}
		}

		if self.box_sync_enabled.load(Ordering::SeqCst)
			&& self.box_synced_workdirs.lock().await.insert(session.workdir.clone())
		{
			let server = self.clone();
			let workdir = session.workdir.clone();
			tokio::spawn(async move {
				server.run_box_sync_background(workdir).await;
			});
		}
		Ok(session)
	}

	/// Background, non-blocking box discovery + sync for one workdir. Logs and swallows
	/// errors so a missing embedding provider or offline remote never disrupts the session.
	async fn run_box_sync_background(&self, workdir: std::path::PathBuf) {
		let provider = match self.get_or_init_knowledge().await {
			Ok(p) => p,
			Err(e) => {
				debug!("Box sync skipped (knowledge init failed): {:?}", e);
				return;
			}
		};
		let projects = match discover_session_projects(workdir).await {
			Ok(projects) => projects,
			Err(e) => {
				debug!("Box sync skipped (project discovery failed): {:?}", e);
				return;
			}
		};
		if let Err(e) = provider.sync_boxes(&projects).await {
			debug!("Box sync failed: {:?}", e);
		}
	}

	async fn get_memory_provider(
		&self,
		session: &SessionState,
		scope: Option<String>,
		role: Option<String>,
	) -> Result<MemoryProvider, McpError> {
		let scope = session.resolve_scope(scope)?;
		let role = if session.role_locked {
			session.role.clone()
		} else {
			role
		};
		let key = (scope.clone(), role.clone());
		let mut providers = self.memory.lock().await;
		if let Some(provider) = providers.get(&key) {
			return Ok(provider.clone());
		}
		let provider = MemoryProvider::new(&self.config, self.working_directory.clone(), scope, role)
			.await
			.map_err(|e| McpError::internal_error(format!("Failed to initialize memory: {}", e), None))?;
		providers.insert(key, provider.clone());
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

		let provider = KnowledgeProvider::new(&self.config)
			.await
			.map_err(|e| McpError::internal_error(format!("Failed to initialize knowledge: {}", e), None))?;

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
		let providers = memory.lock().await.clone();
		for provider in providers.values() {
			if tokio::time::timeout(MAINTENANCE_SHUTDOWN_GRACE, provider.drain_pending_maintenance())
				.await
				.is_err()
			{
				warn!("maintenance still running at shutdown; abandoning it");
			}
		}

		Ok(())
	}

	/// Run server using streamable HTTP transport (advertises MCP 2026-07-28; rmcp's legacy `initialize` serves older clients)
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
	McpError::internal_error(e.message, Some(serde_json::to_value(e.operation).unwrap_or_default()))
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
	Goal,
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
/// advances a Goal memory), closes (this memory completes a Goal — folds all
/// Achieves sources into it as the consolidated parent).
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
	Closes,
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
	/// Scope string to store this memory under (e.g. 'github.com/org/repo'). Omit for the session's project; without a session, omitted means global.
	pub scope: Option<String>,
	/// When true, store this memory in the global scope (shared across all projects) regardless of the locked session scope.
	pub global: Option<bool>,
	/// Role tag to attach to this memory (e.g. 'developer', 'reviewer').
	pub role: Option<String>,
	/// Optional: create typed relationships from this new memory to existing
	/// memories in the same call. Subsumes the standalone relate tool.
	/// Most common use: contributing toward a Goal via
	/// `{ target_id: goal_id, relationship_type: "achieves" }`, then later
	/// closing it with `{ target_id: goal_id, relationship_type: "closes" }`.
	#[schemars(length(max = 20))]
	pub related_to: Option<Vec<RelationshipSpec>>,
}

/// Remember tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RememberParams {
	/// String or array of 2-5 related terms (max 5; drop the weakest extras). Array preferred for broader semantic coverage.
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
	/// Filter by scope. If omitted, returns memories from the session's project plus global ones (all scopes without a session).
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
	/// Scope to delete in. Omit for the session's project.
	pub scope: Option<String>,
	/// Role filter for query deletion
	pub role: Option<String>,
}

/// Command for the knowledge tool: search (semantic search across indexed
/// knowledge), store (store raw text content under a unique key,
/// session-scoped), delete (delete stored content by key), read (read full
/// content of a URL or local file — fallback when search is insufficient),
/// match (search indexed content by regex pattern, like grep), sync (re-index
/// knowledge boxes after files under <repo>/.box/ change).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeAction {
	Search,
	Store,
	Delete,
	Read,
	Match,
	Sync,
}

/// Knowledge tool parameters
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeParams {
	/// Command to execute: search, store, delete, read, match, sync
	pub command: KnowledgeAction,
	/// [search] What to search for, in natural language (required for search)
	#[schemars(length(min = 3, max = 500))]
	pub query: Option<String>,
	/// [search] Source filter — a SINGLE URL or local FILE path to auto-index and search within, or a box:// source from earlier results to search one box file. MUST point to one specific file (e.g. /path/to/notes.md, https://example.com/page) — directories are NOT supported and will be rejected. Supports http/https URLs, file:///path, or /absolute/path. File types: .html, .txt, .md, .pdf, .docx. Never pass a file under .box/ as a path (it indexes a duplicate). Omit to search across ALL indexed sources, knowledge boxes included.
	/// [read] A SINGLE URL or local FILE path to read full content from. MUST point to one specific file — directories are NOT supported. Supports http/https URLs, file:///path, or /absolute/path. File types: .html, .txt, .md, .pdf, .docx. (box:// is not accepted; read the file under .box/ itself)
	/// [match] Source filter — a SINGLE URL, local FILE path or box:// source. MUST point to one specific file — directories are NOT supported. Omit to match across ALL indexed sources, knowledge boxes included.
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
		description = "Store information, insights, or context in memory. Call remember first to avoid duplicates. Set source='user_confirmed' for user-stated facts (importance 0.8-1.0), 'agent_inferred' for AI conclusions (0.3-0.6). Skip transient state or things easily re-derived. For a longer reference document, write a knowledge box file instead (see the knowledge tool).\n\nUse related_to[] to link the new memory to existing ones in the same call. Relationship types: related_to, depends_on, supersedes, similar, conflicts, implements, extends, achieves, closes.\n\nKnowledge updates: when a new fact replaces or corrects an existing memory (a value changed, a decision was reversed), memorize the new fact with related_to=[{target_id: <old_id>, relationship_type: 'supersedes'}]. Retrieval then ranks the current fact above the outdated one, which stays queryable for history. remember first to find the old_id.\n\nGoal workflow:\n1. memorize a 'goal' type memory for the task — captures intent\n2. For each contributing memory: memorize with related_to=[{target_id: goal_id, relationship_type: 'achieves'}]\n3. When the task closes: memorize the completion / lesson-learned note with related_to=[{target_id: goal_id, relationship_type: 'closes'}] (goal_id must be a 'goal' memory with at least one Working achieves source). This triggers automatic consolidation — your closing memo becomes the consolidated parent, all Achieves sources transition to Consolidated state with dampened importance (still queryable for audit). Importance of the closing memo is bumped to max(sources) * 1.1. No separate consolidate call needed.\n\nglobal=true (only available when session scope is locked): stores the memory in the shared global scope instead of the locked project scope. Use this for cross-project facts — user preferences, personal habits, universal conventions, tool choices — things that apply regardless of which project is active. Do NOT use global for project-specific knowledge.",
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
		let mut session = self.ensure_session_context(&context).await?;
		if session.scope_locked && params.global == Some(true) {
			session.scope = Some(String::new());
		}
		let provider = self
			.get_memory_provider(&session, params.scope.clone(), params.role.clone())
			.await?;
		let args = serde_json::to_value(&params)
			.map_err(|e| McpError::internal_error(format!("Failed to serialize params: {}", e), None))?;
		provider.execute_memorize(&args).await.map_err(to_rmcp_error)
	}

	#[tool(
		name = "remember",
		description = "Semantic search over stored memories. Call before memorize to avoid duplicates, and at task start to load context. Results include 1-hop graph neighbors automatically. A project scope also returns global memories. Prefer 2-5 related query terms for broader coverage. Results show [CONFIRMED]/[INFERRED] trust labels. For temporal questions (\"last week\", \"in May\"), set created_after/created_before (ISO-8601) to scope the time window — you know today's date, so compute the bounds.",
		annotations(title = "Remember", read_only_hint = true, open_world_hint = false)
	)]
	async fn remember(
		&self,
		context: RequestContext<RoleServer>,
		Parameters(params): Parameters<RememberParams>,
	) -> Result<String, McpError> {
		let session = self.ensure_session_context(&context).await?;
		let provider = self
			.get_memory_provider(&session, params.scope.clone(), params.role.clone())
			.await?;
		let args = serde_json::to_value(&params)
			.map_err(|e| McpError::internal_error(format!("Failed to serialize params: {}", e), None))?;
		provider.execute_remember(&args).await.map_err(to_rmcp_error)
	}

	#[tool(
		name = "forget",
		description = "Permanently delete memories. Irreversible — requires confirm=true. Use memory_id for single deletion, or query+filters for bulk removal. memory_id deletes any memory remember returned, project or global; query deletes only in the given scope (omitted: the session's project), so a project query never deletes global memories. Don't forget memories just because they're old — importance decay handles that. Only delete when information is wrong; for a replaced fact, memorize the new one with supersedes instead.",
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
		let session = self.ensure_session_context(&context).await?;
		let provider = self
			.get_memory_provider(&session, params.scope.clone(), params.role.clone())
			.await?;
		let args = serde_json::to_value(&params)
			.map_err(|e| McpError::internal_error(format!("Failed to serialize params: {}", e), None))?;
		provider.execute_forget(&args).await.map_err(to_rmcp_error)
	}

	#[tool(
        name = "knowledge",
        description = "Knowledge base with six commands. The 'source' parameter (when used) ALWAYS refers to a SINGLE FILE or URL — never a directory; passing a directory path is an error. URLs are fetched anonymously — public documentation and information pages only, never API endpoints that need credentials (GitHub Actions logs, private repos, etc. return 403; use the platform's own CLI for those). 'search': semantic search across indexed content — provide source (single URL or file) to auto-index on-the-fly, omit to search all indexed sources, knowledge boxes included. 'store': save raw text under a unique key (session-scoped scratch, auto-cleaned, never durable) — error if key exists, delete first to replace. 'delete': remove stored content by key. 'read': fetch and return the FULL text content of a single URL or file — use ONLY as a last resort when search results are insufficient; prefer 'search' for targeted retrieval. 'match': search indexed content by regex pattern (like grep) — returns matching lines only; prefer 'search' for semantic queries, use 'match' for exact string/regex patterns. 'sync': re-index knowledge boxes — run it after you create, edit or delete files under '<repo>/.box/'. Supported file types: .html, .txt, .md, .pdf, .docx.\n\nKnowledge boxes are the durable, shared knowledge base: files under '<repo>/.box/' (one topic per Markdown file, grouped in folders such as rules/, guides/, decisions/; the root README.md is not indexed), plus boxes subscribed with the octobrain CLI. Box results show source 'box://<box_id>/<path>'; for this project's box the file is '<repo>/.box/<path>' — edit it with your file tools, then 'sync'. A box:// source in search or match narrows to that one file; never pass a .box/ file path as source (it indexes a duplicate). One fact or decision belongs in memorize, not a box file.",
        // Mixed commands: `delete` mutates stored content and URL fetch
        // reaches the open world — spec defaults kept deliberately.
        annotations(title = "Knowledge", read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
	async fn knowledge(
		&self,
		context: RequestContext<RoleServer>,
		Parameters(params): Parameters<KnowledgeParams>,
	) -> Result<String, McpError> {
		let session = self.ensure_session_context(&context).await?;
		let provider = self.get_or_init_knowledge().await?;
		let session_id = session.session_id.clone();
		let active_scope = session.scope.clone();

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
					.execute_store(params.key.as_deref(), params.content.as_deref(), &session_id)
					.await
			}
			KnowledgeAction::Delete => provider.execute_delete(params.key.as_deref(), &session_id).await,
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
			KnowledgeAction::Sync => {
				let projects = discover_session_projects(session.workdir.clone()).await?;
				provider.execute_sync(&projects).await
			}
		}
		.map_err(to_rmcp_error)
	}
}

// ============================================================================
// ServerHandler implementation
// ============================================================================

/// 2026-07-28 makes `ttlMs` and `cacheScope` required on list results; older
/// protocol versions don't define them. rmcp's generated `list_tools` sets them,
/// so the override below must too — a strict 2026-07-28 client (Claude Code's
/// `server/discover` runtime) rejects the result and loads no tools at all.
fn supports_cache_hints(context: &RequestContext<RoleServer>) -> bool {
	context
		.protocol_version()
		.is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

/// Name an unlocked session's scope options in the `scope` parameter, so the model
/// picks among the repos under its workdir instead of guessing.
fn with_scope_options(tools: Vec<Tool>, session: &SessionState) -> Vec<Tool> {
	let Some(default_scope) = &session.default_scope else {
		return tools;
	};
	let options = serde_json::json!(session.scope_options());
	let description = serde_json::json!(format!(
		"Project scope. Omit for this session's project ('{}'); '' is the global scope shared by all projects.",
		default_scope
	));
	tools
		.into_iter()
		.map(|mut tool| {
			let mut schema = tool.input_schema.as_ref().clone();
			if let Some(scope) = schema
				.get_mut("properties")
				.and_then(|properties| properties.get_mut("scope"))
				.and_then(|scope| scope.as_object_mut())
			{
				scope.insert("enum".to_string(), options.clone());
				scope.insert("description".to_string(), description.clone());
			}
			tool.input_schema = Arc::new(schema);
			tool
		})
		.collect()
}

#[tool_handler]
impl ServerHandler for McpServer {
	fn get_info(&self) -> ServerConfig {
		ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
			.with_protocol_version(ProtocolVersion::V_2026_07_28)
			.with_server_info(
				Implementation::new("octobrain", env!("CARGO_PKG_VERSION"))
					.with_title("Octobrain Memory Server")
					.with_description("Standalone memory management system for AI context and conversation state"),
			)
			.with_instructions(self.instructions.clone())
	}

	/// Return tool list with scope/role stripped from schemas when session context is known
	async fn list_tools(
		&self,
		_request: Option<PaginatedRequestParams>,
		context: RequestContext<RoleServer>,
	) -> Result<ListToolsResult, McpError> {
		let session = self.ensure_session_context(&context).await?;
		let tools = if session.role_locked && session.scope_locked {
			tools_locked().clone() // strip scope + role
		} else if session.role_locked {
			tools_role_only().clone() // strip role only, scope stays visible
		} else if session.scope_locked {
			tools_scope_only().clone() // strip scope only, role + global stay visible
		} else {
			tools_full_no_global().clone()
		};
		let result = ListToolsResult::with_all_items(with_scope_options(tools, &session));
		if !supports_cache_hints(&context) {
			return Ok(result);
		}
		// The schema depends on this client's session capabilities: never shared.
		Ok(result.with_ttl_ms(0).with_cache_scope(CacheScope::Private))
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
		type AnnotationRow = (&'static str, Option<bool>, Option<bool>, Option<bool>, Option<bool>);
		let expected: &[AnnotationRow] = &[
			("memorize", Some(false), Some(false), Some(false), Some(false)),
			("remember", Some(true), None, None, Some(false)),
			("forget", Some(false), Some(true), Some(true), Some(false)),
			("knowledge", Some(false), Some(true), Some(false), Some(true)),
		];
		for variant in [
			tools_full(),
			tools_full_no_global(),
			tools_locked(),
			tools_role_only(),
			tools_scope_only(),
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
				assert!(!a.title.as_deref().unwrap_or_default().is_empty(), "{name} title");
				assert_eq!(a.read_only_hint, *read_only, "{name} read_only_hint");
				assert_eq!(a.destructive_hint, *destructive, "{name} destructive_hint");
				assert_eq!(a.idempotent_hint, *idempotent, "{name} idempotent_hint");
				assert_eq!(a.open_world_hint, *open_world, "{name} open_world_hint");
			}
		}
	}
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
