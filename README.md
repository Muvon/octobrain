# Octobrain - Standalone Memory Management System

Standalone memory management system for AI context and conversation state.

## Features

- **Semantic Search**: Find memories using natural language queries
- **Vector Storage**: LanceDB for efficient vector similarity search
- **Git Integration**: Automatic context tracking with Git commits
- **Memory Commands**: Complete memory lifecycle management under `memory` subcommand
- **Knowledge Base**: Web content indexing and semantic search for persistent knowledge
- **MCP Server**: Model Context Protocol server exposing memory and knowledge tools
- **Platform-Specific Storage**: Uses standard XDG data directories per OS

## Installation

### From Source

```bash
# Build with default features (fastembed + huggingface)
cargo build --release

# Or build without default features (API-based embeddings only)
cargo build --no-default-features --release

# Or build with specific features
cargo build --no-default-features --features fastembed --release
cargo build --no-default-features --features huggingface --release
cargo build --no-default-features --features "fastembed,huggingface" --release

# Run the binary
./target/release/octobrain --help
```

### Features

Octobrain supports multiple embedding providers through feature flags:

- **fastembed**: Local embedding models via FastEmbed (no API key required)
- **huggingface**: Local embedding models via HuggingFace (no API key required)
- **default**: Both fastembed and huggingface features enabled

When building without default features (`--no-default-features`), you can use API-based embedding providers:
- Voyage AI (requires `VOYAGE_API_KEY`)
- OpenAI (requires `OPENAI_API_KEY`)
- Google (requires `GOOGLE_API_KEY`)
- Jina (requires `JINA_API_KEY`)

Configure your embedding provider in `~/.local/share/octobrain/config.toml`:

```toml
[embedding]
model = "voyage:voyage-3.5-lite"  # or "openai:text-embedding-3-small", etc.
```

## Usage

### CLI Structure

Octobrain has three top-level commands:

- `memory` - Memory management for storing and retrieving information
- `knowledge` - Knowledge base management for web content indexing and search
- `mcp` - Start MCP server (Model Context Protocol) exposing memory and knowledge tools
- `help` - Print this message or help of the given subcommand(s)

### Memory Commands

All memory-related commands are grouped under the `memory` subcommand:

```bash
# Store a memory
octobrain memory memorize --title "API Design" --content "Use REST principles" --tags "api,design"

# Search memories
octobrain memory remember "api design patterns"

# Multiple query search
octobrain memory remember "authentication" "security" "jwt"

# Delete a memory
octobrain memory forget --memory-id <id>

# Update a memory
octobrain memory update <id> --title "New Title" --add-tags "new-tag"

# Get memory by ID
octobrain memory get <id>

# List recent memories
octobrain memory recent --limit 20

# Filter by type
octobrain memory by-type architecture --limit 10

# Search by tags
octobrain memory by-tags "api,security"

# Find memories for files
octobrain memory for-files "src/main.rs,src/lib.rs"

# Show statistics
octobrain memory stats

# Clean up old memories
octobrain memory cleanup

# Create relationships
octobrain memory relate <source-id> <target-id> --relationship-type "depends_on"

# View relationships
octobrain memory relationships <memory-id>

# Find related memories
octobrain memory related <memory-id>
```

### Knowledge Commands

Knowledge base commands for web content indexing and semantic search:

```bash
# Index a URL into knowledge base (fetches and chunks content)
octobrain knowledge index https://example.com/docs

# Search knowledge base
octobrain knowledge search "how to configure authentication"

# Search within a specific URL (auto-indexes if needed)
octobrain knowledge search "authentication" --url https://example.com/docs

# Delete a URL and all its chunks from knowledge base
octobrain knowledge delete https://example.com/docs

# Show knowledge base statistics
octobrain knowledge stats

# List indexed sources
octobrain knowledge list --limit 20
```

### MCP Server

Start the MCP server to expose memory and knowledge tools via Model Context Protocol:

```bash
octobrain mcp
```

The server exposes six tools:

**Memory Tools:**
- `memorize`: Store new memories with metadata (title, content, type, tags, importance)
- `remember`: Semantic search with multi-query support and filters
- `forget`: Delete memories by ID or query (requires confirmation)
- `auto_link`: Find and connect related memories based on semantic similarity
- `memory_graph`: Explore memory relationships with multi-hop graph traversal

**Knowledge Tools:**
- `knowledge_search`: Search indexed web knowledge with optional auto-indexing. Provide a URL to search within that source (auto-indexes if outdated), or omit URL to search across all indexed knowledge.

## Configuration

Configuration is stored in `~/.local/share/octobrain/config.toml` on Unix-like systems.

### Default Configuration

```toml
[embedding]
# Embedding model for memory operations
# Format: provider:model (e.g., voyage:voyage-3.5-lite, openai:text-embedding-3-small)
# Default: voyage:voyage-3.5-lite
model = "voyage:voyage-3.5-lite"

# Batch size for embedding generation (number of texts to process at once)
# Default: 32
batch_size = 32

# Maximum tokens per batch request
# Default: 100000
max_tokens_per_batch = 100000

[search]
# Similarity threshold for memory search (0.0 to 1.0)
# Lower values = more results, higher values = fewer but more relevant
# Default: 0.3
similarity_threshold = 0.3

# Maximum number of results to return from search
# Default: 50
max_results = 50

[search.reranker]
# Enable reranking for improved search accuracy
# Default: false
enabled = false

# Reranker model (fully qualified, e.g., voyage:rerank-2.5)
# Default: voyage:rerank-2.5
model = "voyage:rerank-2.5"

# Number of candidates to retrieve before reranking
# Default: 50
top_k_candidates = 50

# Number of results to return after reranking
# Default: 10
final_top_k = 10
```

## Storage Locations

Memories are stored in platform-specific directories following XDG Base Directory specification:

- **macOS**: `~/.local/share/octobrain/`
- **Linux**: `~/.local/share/octobrain/` (or `$XDG_DATA_HOME/octobrain/`)
- **Windows**: `%APPDATA%\octobrain\`

Project-specific data is stored in subdirectories identified by Git remote URL hash.

## Memory Types

- `code`: Code insights and patterns
- `architecture`: System design decisions
- `bug_fix`: Bug fixes and solutions
- `feature`: Feature implementations
- `documentation`: Documentation and knowledge
- `user_preference`: User settings and preferences
- `decision`: Project decisions
- `learning`: Learning notes and tutorials
- `configuration`: Configuration and setup
- `testing`: Testing strategies
- `performance`: Performance optimizations
- `security`: Security considerations
- `insight`: General insights

## License

Apache-2.0

## Credits

Developed by Muvon Un Limited.
