# Octobrain — Development Guide

Standalone memory management system for AI context and conversation state. Exposes a CLI (`octobrain`) and an MCP server. Built in Rust (1.95, edition 2021) using LanceDB for vector storage, `rmcp` for MCP protocol, and `octolib` for embedding providers. Apache-2.0, by Muvon Un Limited.

## Project Structure

```
src/
  main.rs              — CLI entry point, dispatches to commands.rs
  cli.rs               — Clap structs: Commands, MemoryCommand, KnowledgeCommand
  commands.rs          — execute(), execute_memory_command(), execute_knowledge_command()
  config.rs            — Config structs + load() (strict: no defaults, all from TOML)
  storage.rs           — XDG-compliant storage path resolution
  embedding.rs         — Embedding provider factory (octolib)
  vector_optimizer.rs  — LanceDB index optimization logic
  constants.rs         — Project-wide constants
  lib.rs               — Public re-exports
  memory/
    types.rs           — Memory, MemoryQuery, MemoryRelationship, MemoryConfig, etc.
    manager.rs         — MemoryManager: high-level ops, stale-ref cleanup, auto-link
    store.rs           — MemoryStore: LanceDB tables, hybrid search, RRF fusion
    formatting.rs      — CLI output formatting
    git_utils.rs       — Git commit/remote detection
    mod.rs             — Module exports
    *_tests.rs         — hybrid_tests, decay_tests, auto_link_tests, role_tests, hyde_tests, goal_tests, sleep_tests
  knowledge/
    types.rs           — KnowledgeChunk, KnowledgeSearchResult, IndexResult, etc.
    manager.rs         — KnowledgeManager: index, search, read, match, store, delete
    store.rs           — LanceDB vector storage for knowledge chunks
    chunker.rs         — Parent/child chunking for web content
    content.rs         — URL/file fetching and content extraction
    formatting.rs      — CLI output formatting
    mod.rs             — Module exports
  mcp/
    server.rs          — McpServer: 4 tools via rmcp macros, stdio + HTTP transports
    memory.rs          — MemoryProvider: execute_memorize/remember/forget
    knowledge.rs       — KnowledgeProvider: execute_search/store/delete/read/match
    types.rs           — McpError utilities
    logging.rs         — Server-side logging
    mod.rs             — Module exports
config-templates/
  default.toml         — CANONICAL config template — update here first for any new option
```

## Where to Look

| Task | Start here |
|------|------------|
| Add a memory CLI command | `src/cli.rs` → `MemoryCommand` enum, then `src/commands.rs` → `execute_memory_command()` |
| Add a knowledge CLI command | `src/cli.rs` → `KnowledgeCommand` enum, then `src/commands.rs` → `execute_knowledge_command()` |
| Add/change an MCP tool | `src/mcp/server.rs` (tool macro + params struct), `src/mcp/memory.rs` or `knowledge.rs` |
| Add a config option | `src/config.rs` struct → `config-templates/default.toml` (both, always together) |
| Change memory data model | `src/memory/types.rs` → `src/memory/store.rs` (schema + batch_to_memories) |
| Change knowledge data model | `src/knowledge/types.rs` → `src/knowledge/store.rs` |
| Search/query logic | `src/memory/store.rs` → `search_memories()`, `hybrid_search()`, `vector_search()` |
| Embedding provider | `src/embedding.rs` → `octolib` crate |
| Storage paths | `src/storage.rs` |
| LanceDB index tuning | `src/vector_optimizer.rs` |

## How Things Work

### Core Principles
- **No defaults in code** — all config values come from `config-templates/default.toml`, copied to `~/.local/share/octobrain/config.toml` on first run. `Config::load()` is strict.
- **`--no-default-features` always** — default features enable `fastembed`/`huggingface` (heavy local models). Dev/CI always uses `--no-default-features`.
- **Zero clippy warnings** — `cargo clippy --no-default-features --all-targets -- -D warnings` must pass clean.
- **Copyright header** — every `.rs` file starts with `// Copyright 2026 Muvon Un Limited` then `//`.
- **No `unwrap()`** — use `?` and `Result<T>`. Fail fast with meaningful messages.
- **No blocking in async** — no `std::thread::sleep`, no sync I/O in async contexts.
- **Minimal new deps** — reuse what's in `Cargo.toml` before adding anything.

### Config Pattern
```rust
// ✅ Add to struct in src/config.rs
pub struct MemoryConfig {
    pub my_new_field: u32,
    // ...
}

// ✅ Add Default impl value
fn default() -> Self { Self { my_new_field: 42, ... } }

// ✅ MANDATORY: also add to config-templates/default.toml with comment
# My new field description
# Default: 42
my_new_field = 42
```
Never add a config field without updating `config-templates/default.toml`.

### Memory Storage Pattern
```rust
// ✅ Store via manager (handles embedding + auto-link)
let memory = memory_manager.memorize(MemorizeParams { ... }).await?;

// ✅ Search via MemoryQuery builder
let results = memory_manager.remember(&query, Some(filters)).await?;

// ✅ Query struct — use ..Default::default() for unset fields
let q = MemoryQuery {
    query_text: Some("api design".into()),
    memory_types: Some(vec![MemoryType::Architecture]),
    limit: Some(10),
    ..Default::default()
};

// ❌ Never create index manually — VectorOptimizer handles it
// table.create_index(&["embedding"], Index::Auto)

// ❌ Never hardcode partition counts
// .num_partitions(256)
```

### MCP Tool Pattern
Tools are defined with `#[tool(...)]` macros on `McpServer` in `src/mcp/server.rs`. Each tool has a typed `Params` struct (with `JsonSchema`, `Serialize`, `Deserialize`). Execution delegates to `MemoryProvider` or `KnowledgeProvider`.

```rust
// ✅ Tool definition pattern
#[tool(name = "my_tool", description = "...")]
async fn my_tool(&self, Parameters(params): Parameters<MyToolParams>) -> Result<String, McpError> {
    let provider = self.get_or_init_memory().await?;
    let args = serde_json::to_value(&params).map_err(|e| McpError::internal_error(...))?;
    provider.execute_my_tool(&args).await.map_err(|e| McpError::internal_error(...))
}
```

Session locking: when `session.locked == true`, strip `project`/`role` from args before passing to provider.

### Knowledge Chunking
Parent/child model: large content → parent sections stored as `parent_content` (returned to user), split into child chunks (embedded + matched). Config: `chunk_size = 1200`, `chunk_overlap = 300`.

### Adding a Memory Command (checklist)
1. Add variant to `MemoryCommand` in `src/cli.rs` with `/// doc comment` for help text
2. Add match arm in `execute_memory_command()` in `src/commands.rs`
3. Call `MemoryManager` methods — don't reach into `MemoryStore` directly from commands
4. Follow existing output pattern: `format_memories()` / `format_search_results()`

### Adding a Knowledge Command (checklist)
1. Add variant to `KnowledgeCommand` in `src/cli.rs`
2. Add match arm in `execute_knowledge_command()` in `src/commands.rs`
3. Call `KnowledgeManager` methods

### Adding an MCP Tool (checklist)
1. Define `MyParams` struct in `src/mcp/server.rs` with `JsonSchema + Serialize + Deserialize`
2. Add `#[tool(...)]` method on `McpServer`
3. Add `execute_my_tool()` on `MemoryProvider` or `KnowledgeProvider`
4. Update `get_info()` instructions string if needed

## MCP Server

**4 tools** (knowledge is a unified tool with a `command` discriminator):

| Tool | Purpose |
|------|---------|
| `memorize` | Store a memory with type, title, content, tags, importance, source trust; optional `related_to` for inline relationships |
| `remember` | Semantic search — single query or array of 2-5 terms; returns 1-hop graph neighbors |
| `forget` | Delete by `memory_id` or by query+filters; requires `confirm=true` |
| `knowledge` | Unified: `search`, `store`, `delete`, `read`, `match` via `command` field |

**Transport modes:**
- Stdio (default): `octobrain mcp`
- HTTP: `octobrain mcp --bind=host:port` (streamable HTTP, MCP 2025-03-26)

**Session locking:** project/role injected at `initialize` handshake via experimental capabilities; once locked, per-call overrides are ignored.

## Memory Types

`code` · `architecture` · `bug_fix` · `feature` · `documentation` · `user_preference` · `decision` · `learning` · `configuration` · `testing` · `performance` · `security` · `validation` · `research` · `workflow` · `requirement` · `design` · `integration` · `communication` · `process` · `insight` · `goal`

## Storage

- **macOS/Linux**: `~/.local/share/octobrain/` (XDG; respects `$XDG_DATA_HOME`)
- **Windows**: `%APPDATA%\octobrain\`
- Project-scoped data lives in subdirs keyed by SHA-256 of Git remote URL
- Config: `~/.local/share/octobrain/config.toml`

## Gotchas

- `Config::load()` is strict — missing fields cause startup failure. Always update `config-templates/default.toml` alongside `src/config.rs`.
- `tags` and `memory_types` are stored as JSON strings in LanceDB — they **cannot** be filtered via SQL predicates. Filtering happens in Rust post-fetch (`matches_json_filters()`). Don't push these into LanceDB `only_if()` clauses.
- `MemoryStore` holds `project_key` and `role` at construction time. Scoping is baked in — don't pass them as query params expecting them to override.
- Stale-ref cleanup runs on `MemoryManager::new()` — checks Git HEAD and only re-scans when HEAD changes (marker file in storage dir).
- `auto_link` runs automatically on `memorize` and `update_memory`. Calling it manually is for refresh only.
- The `knowledge` MCP tool is a single tool with a `command` discriminator — not separate tools. The CLI has separate subcommands.
- `--no-default-features` is mandatory for all cargo invocations. Default features pull in large local model dependencies (`fastembed`, `huggingface`).

## Never

- Add a config field without updating `config-templates/default.toml`
- Use `unwrap()` or `expect()` in non-test code
- Run `cargo build` / `cargo check` / `cargo test` without `--no-default-features`
- Create LanceDB indexes manually — `ensure_optimal_index()` / `VectorOptimizer` handles this
- Hardcode LanceDB partition counts or index parameters — optimizer calculates them
- Add new crate dependencies without checking if existing deps already cover the need
- Omit the copyright header (`// Copyright 2026 Muvon Un Limited`) from new `.rs` files
