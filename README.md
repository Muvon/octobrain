# Octobrain

> Persistent memory for AI assistants — store insights, decisions, and knowledge that survives across conversations.

[![Crates.io](https://img.shields.io/crates/v/octobrain.svg)](https://crates.io/crates/octobrain)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)

**Octobrain** gives your AI assistant a long-term memory. Store code insights, architecture decisions, bug fixes, and knowledge — then retrieve them with semantic search in future sessions. Works as a CLI tool or as an MCP server for integration with Claude Desktop and other AI tools.

## Why Octobrain?

AI assistants start every conversation with zero context. You explain your project, your preferences, your decisions — every single time. Octobrain breaks that cycle:

- **Persistent memory** — Insights survive across sessions, not just within them
- **Semantic search** — Find memories by meaning, not exact keywords
- **Auto-linking** — Related memories connect automatically (Zettelkasten-style)
- **Knowledge indexing** — Ingest docs, articles, and files for retrieval
- **MCP integration** — Works with Claude Desktop and other MCP-compatible tools

## Quick Start

```bash
# Install from crates.io
cargo install octobrain

# Store your first memory
octobrain memory memorize --title "API Design Pattern" \
  --content "Use REST for CRUD, GraphQL for complex queries" \
  --memory-type architecture --tags "api,design"

# Search memories
octobrain memory remember "how should I design APIs"

# Start MCP server for Claude Desktop integration
octobrain mcp
```

## Installation

### From crates.io (Recommended)

```bash
cargo install octobrain
```

### From Source

```bash
# Clone and build
git clone https://github.com/muvon/octobrain.git
cd octobrain
cargo build --release

# Binary location
./target/release/octobrain --help
```

### Feature Flags

Octobrain supports multiple embedding providers:

| Flag | Description | API Key Required |
|------|-------------|------------------|
| `fastembed` | Local embeddings via FastEmbed | No |
| `huggingface` | Local embeddings via HuggingFace | No |
| (default) | Both `fastembed` + `huggingface` | No |
| (no features) | API-based: Voyage, OpenAI, Google, Jina | Yes |

```bash
# Build with local embeddings (default, no API keys needed)
cargo build --release

# Build with API-based embeddings only
cargo build --no-default-features --release
```

For API-based embeddings, set the appropriate environment variable:
- `VOYAGE_API_KEY` for Voyage AI
- `OPENAI_API_KEY` for OpenAI
- `GOOGLE_API_KEY` for Google
- `JINA_API_KEY` for Jina

## Usage

### Memory Management

Store and retrieve insights, decisions, and context:

```bash
# Store a memory
octobrain memory memorize --title "API Design" \
  --content "Use REST for CRUD, GraphQL for complex queries" \
  --memory-type architecture --tags "api,design"

# Search memories (semantic search)
octobrain memory remember "api design patterns"

# Multi-query search for broader coverage
octobrain memory remember "authentication" "security" "jwt"

# Get recent memories
octobrain memory recent --limit 20

# Filter by type
octobrain memory by-type architecture --limit 10

# Filter by tags
octobrain memory by-tags "api,security"

# Find memories related to files
octobrain memory for-files "src/main.rs,src/lib.rs"

# Update a memory
octobrain memory update <id> --title "New Title" --add-tags "new-tag"

# Delete a memory
octobrain memory forget --memory-id <id>
```

### Memory Relationships

Connect related memories for context-rich retrieval:

```bash
# Create a relationship between memories
octobrain memory relate <source-id> <target-id> \
  --relationship-type "depends_on" \
  --description "Source requires target to function"

# View relationships for a memory
octobrain memory relationships <memory-id>

# Find related memories through relationships
octobrain memory related <memory-id>

# Auto-link similar memories (Zettelkasten-style)
octobrain memory auto-link <memory-id>

# Explore memory graph
octobrain memory graph <memory-id> --depth 2
```

### Knowledge Base

Index and search web content, docs, and files:

```bash
# Index a URL
octobrain knowledge index https://docs.rs/tokio/latest/tokio/

# Search knowledge base
octobrain knowledge search "how to handle async tasks"

# Search within a specific source (auto-indexes if outdated)
octobrain knowledge search "spawn blocking" --source https://docs.rs/tokio/

# Store raw text content
octobrain knowledge store "meeting-notes" --content "Discussion points..."

# List indexed sources
octobrain knowledge list --limit 20

# Show statistics
octobrain knowledge stats

# Delete a source
octobrain knowledge delete https://example.com/docs

# Delete stored content by key
octobrain knowledge delete-stored "meeting-notes"
```

### MCP Server

Run as an MCP server for integration with Claude Desktop and other AI tools:

```bash
# Start with stdio transport (for Claude Desktop)
octobrain mcp

# Start with HTTP transport (for web-based tools)
octobrain mcp --bind 0.0.0.0:12345
```

**Available MCP Tools:**

| Tool | Description |
|------|-------------|
| `memorize` | Store memories with metadata |
| `remember` | Semantic search with filters |
| `forget` | Delete memories (requires confirmation) |
| `relate` | Create relationships between memories |
| `auto_link` | Auto-connect similar memories |
| `memory_graph` | Explore memory relationships |
| `knowledge_search` | Search indexed knowledge |

See [MCP Integration](#mcp-integration) for Claude Desktop setup.

## Features

- **Semantic Search** — Find memories by meaning using vector embeddings, not exact keyword matches
- **Hybrid Search** — Combines BM25 full-text search with vector similarity for better results
- **Reranking Support** — Optional cross-encoder reranking for 20-35% accuracy improvement
- **Auto-Linking** — Automatically connects semantically similar memories (Zettelkasten-style)
- **Temporal Decay** — Ebbinghaus forgetting curve for importance management
- **Knowledge Indexing** — Ingest URLs, PDFs, docs for retrieval
- **Project Scoping** — Isolate memories per Git project or share across projects
- **Role Filtering** — Tag memories by role (developer, reviewer, etc.)
- **MCP Protocol** — Full MCP 2025-11-25 compliance for AI tool integration

## Configuration

Configuration is stored in `~/.local/share/octobrain/config.toml`. All options have sensible defaults.

### Key Settings

| Section | Option | Default | Description |
|---------|--------|---------|-------------|
| `[embedding]` | `model` | `voyage:voyage-3.5-lite` | Embedding model (provider:model format) |
| `[search]` | `similarity_threshold` | `0.3` | Minimum relevance (0.0-1.0) |
| `[search.hybrid]` | `enabled` | `true` | Enable BM25 + vector fusion |
| `[search.reranker]` | `enabled` | `false` | Enable cross-encoder reranking |
| `[memory]` | `max_memories` | `10000` | Maximum stored memories |
| `[memory]` | `decay_enabled` | `true` | Enable temporal importance decay |
| `[memory]` | `auto_linking_enabled` | `true` | Auto-connect similar memories |
| `[knowledge]` | `chunk_size` | `1200` | Characters per chunk |

### Embedding Providers

```toml
[embedding]
# Local embeddings (no API key needed)
model = "voyage:voyage-3.5-lite"  # Fast, accurate
model = "openai:text-embedding-3-small"  # OpenAI
model = "google:text-embedding-004"  # Google
model = "jina:jina-embeddings-v2"  # Jina
```

### Full Configuration

See [`config-templates/default.toml`](config-templates/default.toml) for all available options with documentation.

## Memory Types

Organize memories by category for better filtering:

| Type | Use For |
|------|---------|
| `code` | Code patterns, solutions, implementations |
| `architecture` | System design, decisions, patterns |
| `bug_fix` | Bug fixes, troubleshooting, solutions |
| `feature` | Feature specs, implementations |
| `documentation` | Docs, explanations, knowledge |
| `user_preference` | Settings, preferences, workflows |
| `decision` | Project decisions, trade-offs |
| `learning` | Tutorials, notes, education |
| `configuration` | Setup, config, deployment |
| `testing` | Test strategies, QA insights |
| `performance` | Optimizations, benchmarks |
| `security` | Vulnerabilities, fixes, considerations |
| `validation` | Idea/product validation, hypothesis testing |
| `research` | Technical/market research, analysis |
| `workflow` | SOPs, playbooks, process descriptions |
| `requirement` | Business requirements, specs, constraints |
| `design` | UI/UX decisions, wireframes, system design |
| `integration` | API integrations, third-party services |
| `communication` | Stakeholder updates, team decisions |
| `process` | Deployment procedures, runbooks, operations |
| `insight` | General insights, tips |

## MCP Integration

### Claude Desktop Setup

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "octobrain": {
      "command": "/path/to/octobrain",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop. Octobrain tools will be available in your conversations.

### HTTP Transport

For web-based integrations:

```bash
octobrain mcp --bind 0.0.0.0:12345
```

The server exposes endpoints at `/mcp` for MCP protocol communication.

## Storage Locations

Data is stored in platform-specific directories:

| Platform | Location |
|----------|----------|
| macOS | `~/.local/share/octobrain/` |
| Linux | `~/.local/share/octobrain/` or `$XDG_DATA_HOME/octobrain/` |
| Windows | `%APPDATA%\octobrain\` |

Project-specific memories are isolated by Git remote URL hash.

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Run `cargo clippy` and fix all warnings
4. Run `cargo test --no-default-features`
5. Submit a pull request

### Development Setup

```bash
# Clone and build
git clone https://github.com/muvon/octobrain.git
cd octobrain
cargo build --no-default-features

# Run tests
cargo test --no-default-features

# Run clippy
cargo clippy --no-default-features
```

## License

Apache-2.0 — see [LICENSE](LICENSE) for details.

## Credits

Developed by [Muvon Un Limited](https://muvon.io).
