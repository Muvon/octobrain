# Octobrain - Standalone Memory Management System

Standalone memory management system for AI context and conversation state.

## Features

- **Semantic Search**: Find memories using natural language queries
- **Vector Storage**: LanceDB for efficient vector similarity search
- **Git Integration**: Automatic context tracking with Git commits
- **Memory Commands**: Complete memory lifecycle management under `memory` subcommand
- **MCP Server**: Model Context Protocol server exposing memory tools
- **Platform-Specific Storage**: Uses standard XDG data directories per OS

## Installation

```bash
cargo build --release
./target/release/octobrain --help
```

## Usage

### CLI Structure

Octobrain has three top-level commands:

- `memory` - Memory management for storing and retrieving information
- `mcp` - Start MCP server (Model Context Protocol) exposing memory tools
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

### MCP Server

Start the MCP server to expose memory tools via Model Context Protocol:

```bash
octobrain mcp
```

The server exposes three tools:
- `memorize`: Store new memories
- `remember`: Semantic search with multi-query support
- `forget`: Delete memories by ID or query

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
