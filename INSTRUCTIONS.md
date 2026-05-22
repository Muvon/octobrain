# Octobrain — Development Guide

Standalone memory management system for AI context and conversation state. Exposes a CLI (`octobrain`) and an MCP server. Built in Rust (1.95, edition 2021) using LanceDB for vector storage, `rmcp` for MCP protocol, and `octolib` for embedding/reranking. Apache-2.0, by Muvon Un Limited. v0.6.1.

## Project Structure

```
src/
  main.rs              — CLI entry point, dispatches to commands.rs
  cli.rs               — Clap structs: Commands, MemoryCommand, KnowledgeCommand
  commands.rs          — execute(), execute_memory_command(), execute_knowledge_command()
  config.rs            — Config structs + load() (strict: all fields must exist in TOML)
  storage.rs           — XDG-compliant storage path resolution
  embedding.rs         — Embedding provider factory (octolib)
  vector_optimizer.rs  — LanceDB index optimization logic
  constants.rs         — Project-wide constants
  lib.rs               — Public re-exports
  memory/
    types.rs           — Memory, MemoryQuery, MemoryRelationship, MemoryConfig, MemoryDecay, etc.
    manager.rs         — MemoryManager: memorize/remember/forget, auto-link, consolidation, sleep
    store.rs           — MemoryStore: LanceDB tables, hybrid search, RRF fusion, HyDE expansion
    formatting.rs      — format_memories_as_text, format_memories_as_markdown,
                         format_memories_for_cli, format_plain_memories_for_cli
    git_utils.rs       — Git commit/remote detection
    reranker_integration.rs — Wraps octolib reranker for MemorySearchResult re-ranking
    mod.rs             — Module exports
    *_tests.rs         — hybrid, decay, auto_link, role, hyde, goal, sleep test files
  knowledge/
    types.rs           — KnowledgeChunk, KnowledgeSearchResult, IndexResult, etc.
    manager.rs         — KnowledgeManager: index, search, read, match, store, delete
    store.rs           — LanceDB vector storage for knowledge chunks
    chunker.rs         — Parent/child chunking for web content
    content.rs         — URL/file fetching and content extraction
    formatting.rs      — CLI output formatting
    mod.rs             — Module exports
  mcp/
    server.rs          — McpServer: 4 tools via rmcp macros, stdio + HTTP transports, SessionState
    memory.rs          — MemoryProvider: execute_memorize/remember/forget
    knowledge.rs       — KnowledgeProvider: execute_search/store/delete/read/match
    types.rs           — McpError utilities
    logging.rs         — Server-side logging
    mod.rs             — Module exports
config-templates/
  default.toml         — CANONICAL config template — update here first for any new option
```

## Where to Look

| Task / Area | Start here |
|-------------|------------|
| Add a memory CLI command | `src/cli.rs` → `MemoryCommand` enum, then `src/commands.rs` → `execute_memory_command()` |
| Add a knowledge CLI command | `src/cli.rs` → `KnowledgeCommand` enum, then `src/commands.rs` → `execute_knowledge_command()` |
| Add/change an MCP tool | `src/mcp/server.rs` (tool macro + params struct), `src/mcp/memory.rs` or `knowledge.rs` |
| Add a config option | `src/config.rs` struct → `config-templates/default.toml` (both, always together) |
| Change memory data model | `src/memory/types.rs` → `src/memory/store.rs` (schema + batch_to_memories) |
| Change knowledge data model | `src/knowledge/types.rs` → `src/knowledge/store.rs` |
| Search / query logic | `src/memory/store.rs` → `search_memories()`, `hybrid_search()`, `vector_search()` |
| HyDE query expansion | `src/memory/store.rs` → `expand_query_embedding()` (Rocchio blend, no LLM) |
| Reranking | `src/memory/reranker_integration.rs` + `src/config.rs` → `RerankerConfig` |
| Goal consolidation | `src/memory/manager.rs` → `consolidate_goal()` |
| Sleep consolidation | `src/memory/manager.rs` → `sleep_consolidate()`, `maybe_sleep_consolidate()` |
| Stale-ref cleanup | `src/memory/manager.rs` → `cleanup_stale_references()` |
| Embedding provider | `src/embedding.rs` → `octolib` crate |
| Storage paths | `src/storage.rs` |
| LanceDB index tuning | `src/vector_optimizer.rs` |
| CLI output formatting | `src/memory/formatting.rs` (memory), `src/knowledge/formatting.rs` (knowledge) |

## How Things Work

### Core Principles
- **Config comes from TOML** — `Config::load()` is strict: every field must exist in the installed `config.toml`. Rust structs have Default impls for construction only; `config-templates/default.toml` is the source of truth for what ships.
- **`--no-default-features` always** — default features enable `fastembed`/`huggingface` (heavy local models). All dev/CI invocations use `--no-default-features`.
- **Full Apache license header** — every `.rs` file starts with the full 13-line Apache-2.0 license block (copy from any existing `.rs` file). Copyright year: 2026.
- **No `unwrap()` / `expect()`** — use `?` and `Result<T>` everywhere except test code.
- **No blocking in async** — no `std::thread::sleep`, no sync I/O in async contexts.
- **Minimal new deps** — reuse what's in `Cargo.toml` before adding anything.

### Config Pattern
```rust
// ✅ Add field to struct in src/config.rs with Default impl
pub struct MemoryConfig {
    pub my_new_field: u32,
}
impl Default for MemoryConfig {
    fn default() -> Self { Self { my_new_field: 42, ... } }
}

// ✅ MANDATORY: also add to config-templates/default.toml with comment
# My new field description
# Default: 42
my_new_field = 42
```
Never add a config field without updating `config-templates/default.toml`.

### Memory Storage Pattern
```rust
// ✅ Store via manager (handles embedding + fires async auto-link)
// Note: manager::MemorizeParams has NO related_to — relationships are
// created separately via create_relationship() (or inline at the MCP layer).
let memory = memory_manager.memorize(MemorizeParams {
    memory_type, title, content, tags, related_files, importance, source
}).await?;

// ✅ Single-query search
let results = memory_manager.remember(&query, Some(filters)).await?;

// ✅ Multi-query search (merges results with relevance dedup; limit inside MemoryQuery)
let results = memory_manager.remember_multi(&queries, Some(filters)).await?;

// ✅ MemoryQuery — use ..Default::default() for unset fields
let q = MemoryQuery {
    query_text: Some("api design".into()),
    memory_types: Some(vec![MemoryType::Architecture]),
    limit: Some(10),
    ..Default::default()
};

// ❌ Never create LanceDB indexes manually — VectorOptimizer handles it
// ❌ Never hardcode partition counts
// ❌ Never call MemoryStore directly from commands — go through MemoryManager
```

### Search Pipeline (in order)
1. **HyDE expansion** (`expand_query_embedding`) — retrieves top-K neighbors, blends centroid with original embedding (Rocchio, no LLM). Config: `[search.hyde]`.
2. **Hybrid search** (`hybrid_search`) — LanceDB `execute_hybrid()` fuses vector + BM25 via RRF (k=60). Config: `[search.hybrid]`.
3. **Post-fetch Rust filtering** — `tags`/`memory_types` filtered here (JSON strings, not SQL-filterable).
4. **Reranking** (`RerankerIntegration`) — cross-encoder re-scores top-K candidates. Config: `[search.reranker]`, default model `fastembed:jina-reranker-v2-base-multilingual`.
5. **Access recording** — `record_accesses_best_effort()` bumps access count + decay boost after every search.

### Memory Lifecycle
**States:** `Working` (default, active in retrieval) → `Consolidated` (post goal-closure, dampened importance, kept for audit) → `Archived` (manual tombstone before hard delete)

**Source trust multipliers:**
- `user_confirmed` → 1.0 · `imported` → 0.9 · `agent_inferred` → 0.85 · `auto_linked` → 0.8

**Decay:** Ebbinghaus curve — `decay_half_life_days = 90`, floor `min_importance_threshold = 0.05`. Each access boosts importance (`access_boost_factor = 1.2`).

**Auto-maintenance:** background `JoinHandle` fires every `MAINTENANCE_EVERY_N_WRITES = 250` writes — runs `run_maintenance()` (index optimization + compaction). Non-blocking.

### Goal Consolidation
```rust
// ✅ Consolidate a goal — folds all Achieves-linked sources into a parent
// parent_id = Some(id): promote that existing memory (memorize-with-Closes path)
// parent_id = None:     synthesize a fresh Insight as parent (CLI admin path)
memory_manager.consolidate_goal(goal_id, parent_id, summary).await?;
// Sources: state → Consolidated, importance *= 0.2
// Parent: importance = max(sources) * 1.1, clamped to [0.0, 1.0]
```

The `related_to: Closes` relationship in `memorize` triggers consolidation automatically at the MCP layer — the just-stored memory becomes `parent_id`. No manual call needed from the MCP path.

### Sleep Consolidation
Runs automatically at `MemoryManager::new()` via marker-file gating. Finds clusters of similar `Working`-state memories (cosine ≥ threshold, age ≤ `sleep_consolidation_max_age_days`) and folds each cluster via the same goal pipeline. Config: `[memory]` `sleep_consolidation_*` keys. Never call manually in production paths.

### MCP Tool Pattern
Tools are defined with `#[tool(...)]` macros on `McpServer` in `src/mcp/server.rs`. Each tool has a typed `Params` struct (`JsonSchema + Serialize + Deserialize`). Execution delegates to `MemoryProvider` or `KnowledgeProvider`.

```rust
// ✅ Tool definition pattern
#[tool(name = "my_tool", description = "...")]
async fn my_tool(&self, Parameters(params): Parameters<MyToolParams>) -> Result<String, McpError> {
    let provider = self.get_or_init_memory().await?;
    let mut args = serde_json::to_value(&params).map_err(|e| McpError::internal_error(...))?;
    // Session locking: strip project/role when locked
    let session = self.session.lock().await;
    if session.locked {
        if let Some(obj) = args.as_object_mut() { obj.remove("project"); obj.remove("role"); }
    }
    provider.execute_my_tool(&args).await.map_err(|e| McpError::internal_error(...))
}
```

### Knowledge Chunking
Parent/child model: large content → parent sections stored as `parent_content` (returned to user), split into child chunks (embedded + matched). Config: `chunk_size = 1200`, `chunk_overlap = 300`.

### Adding a Memory Command (checklist)
1. Add variant to `MemoryCommand` in `src/cli.rs` with `/// doc comment` for help text
2. Add match arm in `execute_memory_command()` in `src/commands.rs`
3. Call `MemoryManager` methods — don't reach into `MemoryStore` directly from commands
4. Format output with `format_memories_for_cli()` (search results) or `format_plain_memories_for_cli()` (plain `Memory` slice)

### Adding a Knowledge Command (checklist)
1. Add variant to `KnowledgeCommand` in `src/cli.rs`
2. Add match arm in `execute_knowledge_command()` in `src/commands.rs`
3. Call `KnowledgeManager` methods

### Adding an MCP Tool (checklist)
1. Define `MyParams` struct in `src/mcp/server.rs` with `JsonSchema + Serialize + Deserialize`
2. Add `#[tool(...)]` method on `McpServer` with session-locking for `project`/`role`
3. Add `execute_my_tool()` on `MemoryProvider` or `KnowledgeProvider`
4. Update `get_info()` instructions string if needed

## MCP Server

**4 tools** (`knowledge` is a unified tool with a `command` discriminator):

| Tool | Purpose |
|------|---------|
| `memorize` | Store a memory: type, title, content, tags, importance, source trust; `related_to` for inline relationships |
| `remember` | Semantic search — single string or array of 2-5 terms; returns 1-hop graph neighbors |
| `forget` | Delete by `memory_id` or query+filters; requires `confirm=true` |
| `knowledge` | Unified: `search`, `store`, `delete`, `read`, `match` via `command` field |

**Transport modes:**
- Stdio (default): `octobrain mcp`
- HTTP: `octobrain mcp --bind=host:port` (streamable HTTP, MCP 2025-03-26)

**Session locking:** `project`/`role` injected at `initialize` handshake via experimental capabilities; once `session.locked == true`, per-call overrides are stripped before reaching providers.

## Memory Types

`code` · `architecture` · `bug_fix` · `feature` · `documentation` · `user_preference` · `decision` · `learning` · `configuration` · `testing` · `performance` · `security` · `validation` · `research` · `workflow` · `requirement` · `design` · `integration` · `communication` · `process` · `insight` · `goal`

## Storage

- **macOS/Linux**: `~/.local/share/octobrain/` (XDG; respects `$XDG_DATA_HOME`)
- **Windows**: `%APPDATA%\octobrain\`
- Project-scoped data lives in subdirs keyed by SHA-256 of Git remote URL
- Config: `~/.local/share/octobrain/config.toml` (copied from `config-templates/default.toml` on first run)

### Quality criteria
- Zero clippy warnings under `--no-default-features`
- All test files in `src/memory/*_tests.rs` pass
- No `unwrap()` / `expect()` outside `*_tests.rs`
- Every new `.rs` file carries the full Apache-2.0 license header (13 lines)
- `config-templates/default.toml` updated alongside any `src/config.rs` change

## Gotchas

- `Config::load()` is strict — missing fields cause startup failure. Always update `config-templates/default.toml` alongside `src/config.rs`.
- `tags` and `memory_types` are stored as JSON strings in LanceDB — **not SQL-filterable**. Filtering happens in Rust post-fetch (`matches_json_filters()`). Never push them into LanceDB `only_if()` clauses.
- `MemoryStore` holds `project_key` and `role` at construction time — scoping is baked in. Don't pass them as query params expecting override.
- Stale-ref cleanup runs on `MemoryManager::new()` — checks Git HEAD via marker file, only re-scans when HEAD changes.
- `auto_link` fires asynchronously on `memorize` and `update_memory` (background task). Calling `auto_link_memory()` manually is for refresh only.
- The `knowledge` MCP tool is one tool with a `command` discriminator — not separate tools. CLI has separate subcommands.
- Sleep consolidation also runs on `MemoryManager::new()` — gated by marker file and `sleep_consolidation_interval_hours`. Never force-run in production paths.
- Background maintenance fires every 250 writes and is non-blocking. Call `drain_pending_maintenance()` in tests or shutdown paths where ordering matters.
- `format_memories()` and `format_search_results()` do **not** exist. Use `format_memories_for_cli()` (search results) or `format_plain_memories_for_cli()` (plain `Memory` slice).

## Never

- Add a config field without updating `config-templates/default.toml`
- Use `unwrap()` or `expect()` in non-test code
- Create LanceDB indexes manually — `ensure_optimal_index()` / `VectorOptimizer` handles this
- Hardcode LanceDB partition counts or index parameters — optimizer calculates them
- Push `tags` or `memory_types` filters into LanceDB SQL predicates — filter post-fetch in Rust
- Add new crate dependencies without checking if existing deps already cover the need
- Omit the full Apache-2.0 license header from new `.rs` files
