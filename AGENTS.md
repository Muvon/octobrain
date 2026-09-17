# Octobrain — AGENTS.md

Standalone memory management system for AI context and conversation state: a CLI (`octobrain`) plus an MCP server, in Rust (MSRV 1.95, edition 2021). LanceDB vector storage, `rmcp` for MCP, `octolib` for embeddings/reranking. Apache-2.0, Muvon Un Limited.

## Commands

Two build modes, both must stay green: **fast loop** `--no-default-features` (no heavy local models; what Makefile targets use) and **full gate** `--all-features` (what CI + pre-commit run; also compiles `beir_bench`; needs `protoc` on PATH).

- Dev build: `cargo build --no-default-features` · Release: `cargo build --no-default-features --release`
- Check: `cargo check --no-default-features --all-targets` · All feature combos: `make check-features`
- Format (write): `cargo fmt --all` · Format (verify): `cargo fmt --all -- --check` — `make fmt` is verify-only
- Lint: `cargo clippy --no-default-features --all-targets -- -D warnings` (CI/pre-commit variant uses `--all-features`)
- Tests light: `cargo test --no-default-features` · CI parity: `cargo test --all-features` · One test: `cargo test --no-default-features <test_name>`
- Coverage: `cargo llvm-cov --summary-only --all-features --ignore-filename-regex '_tests\.rs$'`
- Run CLI: `cargo run --no-default-features -- <subcommand>` · MCP server: `cargo run --no-default-features -- mcp` (stdio) or `-- mcp --bind=host:port` (streamable HTTP)
- BEIR retrieval bench (local, free): `bash benches/scripts/run_retrieval.sh` or `cargo run --release --features bench --bin beir_bench -- <dataset_dir> [label]`
- Completions: `make install-completions` / `make test-completions`
- `make ci` references nonexistent targets (`format-check`, `lint`) — run the four commands above instead

## Where to look

| Task | Start here |
|------|------------|
| Memory CLI command | `src/cli.rs` (`MemoryCommand`) → `src/commands.rs` (`execute_memory_command`) |
| Knowledge / box CLI command | `src/cli.rs` (`KnowledgeCommand`, `BoxCommand`) → `src/commands.rs` |
| MCP tool | `src/mcp/server.rs` (`#[tool]` macro + Params struct) → `src/mcp/memory.rs` / `src/mcp/knowledge.rs` provider |
| Config option | `src/config.rs` struct + `config-templates/default.toml` — always both, same commit |
| Memory data model | `src/memory/types.rs` (incl. `MemoryType`, relationship enums) → `src/memory/store.rs` (schema, `batch_to_memories`) |
| Knowledge data model | `src/knowledge/types.rs` → `src/knowledge/store.rs` |
| Search internals | `src/memory/store.rs`: `search_memories` / `hybrid_search` / `expand_query_embedding` |
| Consolidation | `src/memory/manager.rs`: `consolidate_goal`, `sleep_consolidate` |
| Reranking | `src/memory/reranker_integration.rs` + `RerankerConfig` in `src/config.rs` |
| Cross-process embedding sharing | `src/embedding.rs` + `src/embedding/shared.rs` (election, loopback attach) |
| Knowledge boxes | `src/knowledge/boxes.rs` (registry/git/scope) → `src/knowledge/manager.rs` (`import_box` / `sync_boxes`) |
| Storage paths | `src/storage.rs` · LanceDB index tuning: `src/vector_optimizer.rs` |
| CLI output | `src/memory/formatting.rs` (memory), `src/knowledge/formatting.rs` (knowledge) |
| Benchmarks | `benches/README.md` — BEIR is local/metric-only; LongMemEval is Docker + `.env` + paid LLM calls |
| Recent changes | `CHANGELOG.md` |

## Conventions

- Every new `.rs` file starts with the full 13-line Apache-2.0 header, copyright 2026 (copy from any existing file).
- No `unwrap()` / `expect()` outside `*_tests.rs` — use `?`. Only proven-invariant exceptions exist (in `src/embedding.rs`, `src/reranker.rs`, `src/storage.rs`, `src/mcp/server.rs`, `src/embedding/shared.rs`, `src/memory/store.rs`, `src/memory/formatting.rs`, `src/knowledge/store.rs`); new code needs `?` or a documented invariant.
- Tests live in sibling `*_tests.rs` files, not inline `#[cfg(test)]` modules.
- Config is strict: `Config::load()` fails if any field is missing from the installed TOML. Rust `Default` impls are for construction only — `config-templates/default.toml` is what ships. Shape changes need a step in `plan()` with `version` bumped in the same commit.
- Commands and MCP handlers go through `MemoryManager` / `KnowledgeManager` — never call the stores directly. `MemorizeParams` has no `related_to`; relationships come via `create_relationship()` or inline at the MCP layer.
- MCP tool pattern: typed Params struct (`JsonSchema + Serialize + Deserialize`) → `#[tool(...)]` method on `McpServer` → strip `project`/`role` from args when `session.locked` → `execute_*()` on the provider. Update the `get_info()` instructions string when tool behavior changes.
- Tool JSON schemas are deliberately flattened (`$ref` inlined, null branches stripped) for backend compatibility — do not reintroduce `$ref` or nullable types.
- Memory formatting entry points: `format_memories_for_cli()` (search results) and `format_plain_memories_for_cli()` (plain `&[Memory]`). `format_memories()` / `format_search_results()` do not exist.
- No blocking in async: no `std::thread::sleep`, no sync I/O in async contexts.
- Minimal new deps — reuse `Cargo.toml` first. Its pins are load-bearing (`time =0.3.47`, `alloc-stdlib =0.2.2`, `brotli-decompressor =5.0.1`, `[patch.crates-io]` esaxx-rs git rev); a casual `cargo update` breaks the build (E0119 / E0277 / MSVC LNK2038).
- Release version lives in three places — `Cargo.toml`, `server.json`, `CHANGELOG.md` — update them together (release.yml does not touch `server.json`).

## System behavior (semantics not to break)

- **Search pipeline (memory, in order):** HyDE/PRF query expansion (`[search.hyde]`, Rocchio blend, no LLM) → hybrid vector+BM25 via RRF k=60 (`[search.hybrid]`) → post-fetch Rust filtering of `tags`/`memory_types` → cross-encoder rerank (`[search.reranker]`) → access recording (count + decay boost).
- **States:** `Working` → `Consolidated` (post goal-closure, importance ×0.2, kept for audit) → `Archived` (tombstone before hard delete). Source trust: `user_confirmed` 1.0 · `imported` 0.9 · `agent_inferred` 0.85 · `auto_linked` 0.8. Decay: Ebbinghaus, half-life 90d, floor 0.05, access boost ×1.2.
- **Goal consolidation:** parent importance = max(sources) × 1.1 clamped to [0,1]. An MCP-layer `related_to: Closes` triggers it automatically with the just-stored memory as parent — no manual call from the MCP path.
- **`Supersedes` edges** are honored (relevance ×0.1, soft — never deleted) but never auto-created.
- **Background work on `MemoryManager::new()` / writes:** stale-ref cleanup (marker-gated on Git HEAD, rename-aware), sleep consolidation (marker-gated, `[memory] sleep_consolidation_*`), maintenance every 250 writes or 24h (1h retry lease). Never force-run in production paths; use `octobrain memory maintenance` for a synchronous pass and `drain_pending_maintenance()` in tests/shutdown.
- **auto_link** fires asynchronously on `memorize`/`update_memory`; `auto_link_memory()` is refresh-only; `consolidate_goal` drains pending links first.
- **`remember`** accepts 1 string or 2–5 terms (array); `remember_multi` fuses per-query results with RRF k=60.
- **Knowledge boxes** ship sources only, never vectors; rows carry `box://<box_id>/<rel>` URIs and are pruned when files leave the box. Project-local `.box/` dirs are discovered at sync, never registered; global boxes have empty scope.
- **Chunking:** parent sections (returned to user) split into child chunks (embedded); `chunk_size = 1200`, `chunk_overlap = 300`.
- **Storage:** one shared LanceDB scoped by the `scope` column (normalized Git remote URL) — not per-project dirs. `~/.local/share/octobrain/` (XDG) / `%APPDATA%\octobrain\`; boxes under `<storage>/boxes/`, shared-embedding endpoints under `<storage>/run/`.

## Done

- `cargo fmt --all -- --check` exits 0
- `cargo clippy --all-targets --all-features -- -D warnings` exits 0
- `cargo test --all-features` passes (CI matrix: ubuntu/windows/macos on Rust 1.98.0)
- `make check-features` passes when touching feature-gated code
- Non-command criteria: new config fields shipped in `config-templates/default.toml`; new files carry the license header; no new `unwrap()`/`expect()` outside tests

## Gotchas

- `tags` and `memory_types` are stored as JSON strings in LanceDB — **not SQL-filterable**; filter post-fetch in Rust (`matches_json_filters()`), never in `only_if()` clauses.
- `MemoryStore` bakes `project_key` and `role` at construction — they are not query-time overrides.
- The `knowledge` MCP tool is one tool with a `command` discriminator; the CLI has separate subcommands.
- Full-feature builds need `protoc`. Linux test runs need `ORT_LIB_LOCATION` (static ONNX Runtime); Windows needs `RUSTFLAGS=-C target-feature=-crt-static`. macOS works out of the box. See `.github/workflows/ci.yml`.
- Env `RUSTFLAGS` replaces `.cargo/config.toml` entirely — re-apply target flags (musl `+fp16`/`-lgcc`) when setting it.
- MSRV is 1.95 (`rust-version`); CI pins 1.98.0 — do not use newer APIs.
- Stale Makefile details: `make fmt` verifies only, `make ci` is broken, `make install-deps` has a `trustup` typo. Prefer raw cargo commands.
- LongMemEval benches require `benches/.env` (LLM keys) and cost ~10M tokens per full run; smoke-test with `make smoke` in `benches/`.

## Never

- Add a config field without updating `config-templates/default.toml` (and `plan()` when the shape changes)
- Use `unwrap()` or `expect()` in non-test code
- Create LanceDB indexes manually or hardcode partition counts — `VectorOptimizer` / `ensure_optimal_index()` owns this
- Push `tags` / `memory_types` filters into LanceDB SQL predicates
- Call `MemoryStore` / `KnowledgeStore` directly from commands or MCP handlers
- Add dependencies without reusing existing ones, or loosen the pinned versions in `Cargo.toml`
- Ship a `.rs` file without the full Apache-2.0 header
- Force-run sleep consolidation or maintenance in production paths

## References

- `benches/README.md` — read before touching benchmarks (BEIR vs LongMemEval, env knobs, cost estimates)
- `README.md` (§ Configuration, § MCP Integration) — user-facing docs to keep in sync when config or tools change
- `.github/workflows/ci.yml` — canonical test matrix and the ORT/protoc/RUSTFLAGS setup per platform
- `config-templates/default.toml` — annotated source of truth for every option
