# Octobrain Development Instructions

## Core Principles

### Strict Configuration Management
- **NO DEFAULTS**: All configuration must be explicitly defined in `config-templates/default.toml`
- **Template-First**: Update template file when adding new config options
- **Environment Override**: Use env vars for sensitive data (API keys)
- **Version Control**: Config has version field for future migrations

### Code Reuse & Architecture
- **DRY Principle**: Don't repeat yourself - reuse existing patterns
- **KISS Principle**: Keep it simple, stupid - avoid over-engineering
- **Zero Warnings**: All code must pass `cargo clippy` without warnings
- **Fail Fast**: Validate inputs early and return clear error messages

## Project Structure

### Core Modules
- `src/cli.rs` - CLI argument parsing with `memory` subcommand grouping
- `src/commands.rs` - Command execution logic
- `src/config.rs` - Configuration loading and management
- `src/storage.rs` - Platform-specific storage paths (XDG compliant)
- `src/memory/` - Memory management system
  - `types.rs` - Memory data structures
  - `manager.rs` - High-level memory operations
  - `store.rs` - LanceDB vector storage
  - `formatting.rs` - Output formatting utilities
- `src/embedding.rs` - Embedding generation (via octolib)
- `src/mcp/` - Model Context Protocol server
- `src/constants.rs` - Project constants

### CLI Structure
Octobrain uses a hierarchical command structure:

**Root Level Commands** (only 3):
- `memory` - Memory management for storing and retrieving information
- `mcp` - Start MCP server (Model Context Protocol) exposing memory tools
- `help` - Print this message or help of the given subcommand(s)

**Memory Subcommands** (all 16):
- `memorize` - Store important information, insights, or context in memory
- `remember` - Search and retrieve stored memories using semantic search
- `forget` - Permanently remove specific memories
- `update` - Update an existing memory
- `get` - Get memory by ID
- `recent` - List recent memories
- `by-type` - Get memories by type
- `for-files` - Get memories related to files
- `by-tags` - Get memories by tags
- `current-commit` - Get memories for current Git commit
- `stats` - Show memory statistics
- `cleanup` - Clean up old memories
- `clear-all` - Clear ALL memory data (DANGEROUS: deletes everything)
- `relate` - Create a relationship between two memories
- `relationships` - Get relationships for a memory
- `related` - Get related memories through relationships

## Configuration

### Config File Location
Configuration is stored in `~/.local/share/octobrain/config.toml` on Unix-like systems (XDG compliant).

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

## Development Workflow

### MANDATORY BUILD COMMANDS
- **ALWAYS use `--no-default-features`** for ALL cargo commands during development
  ```bash
  cargo build --no-default-features
  cargo check --no-default-features --message-format=short
  cargo test --no-default-features
  ```
- **NEVER use `--release`** unless explicitly requested
- **NEVER use default cargo build** - ALWAYS add `--no-default-features` flag

### Code Quality Standards
- **Zero clippy warnings** - All code must pass `cargo clippy` without warnings
- **Minimal dependencies** - Reuse existing dependencies before adding new ones
- **Clone trait** - Add `#[derive(Clone)]` to structs that need to be shared across async contexts
- **Error handling** - Use proper `Result<T>` types and meaningful error messages

### Testing Approach
- **Unit tests** for individual components
- **Integration tests** for full workflows
- **Manual testing** with real projects during development

## Memory System Architecture

### Memory Storage Pattern
```rust
// ✅ GOOD: Use optimized store methods
store.store_code_blocks(&blocks, &embeddings).await?;
store.store_text_blocks(&blocks, &embeddings).await?;
store.store_document_blocks(&blocks, &embeddings).await?;

// ✅ GOOD: Search with optimized parameters (automatic)
let results = memory_store.search_memories(&query).await?;

// ❌ AVOID: Manual index creation (optimizer handles this)
// table.create_index(&["embedding"], Index::Auto) // Don't do this

// ❌ AVOID: Fixed parameters (optimizer calculates optimal values)
// .num_partitions(256) // Don't hardcode
```

### Memory Query Pattern
```rust
// Build query with filters
let memory_query = MemoryQuery {
    query_text: Some(query.to_string()),
    memory_types: Some(vec![MemoryType::Code]),
    tags: Some(vec!["api".to_string()]),
    limit: Some(10),
    min_relevance: Some(0.5),
    ..Default::default()
};
```

## MCP Server

### Server Modes
- **Stdin Mode** (default): Standard MCP protocol over stdin/stdout for AI assistant integration
- **HTTP Mode** (`--bind=host:port`): HTTP server for web-based integrations and testing

### MCP Tools
The server exposes three tools:
- `memorize`: Store new memories
- `remember`: Semantic search with multi-query support
- `forget`: Delete memories by ID or query

## Quick Start Checklist

1. **Config First**: Always update `config-templates/default.toml` when adding new config options
2. **No Defaults**: Explicit configuration for all options
3. **Reuse Patterns**: Follow existing indexer/storage patterns
4. **Batch Processing**: Use established batch sizes and flush cycles
5. **Git Integration**: Leverage commit-based optimization
6. **Test Incrementally**: Use watch mode for development iteration
7. **Clean Code**: Always run clippy before finalizing code
8. **Zero Warnings**: Ensure `cargo clippy` passes without warnings

## Development Patterns

### Adding New Features
1. Update struct in `src/config.rs` if adding config options
2. Add defaults in `Default` impl if needed
3. **MANDATORY**: Update `config-templates/default.toml`
4. Add validation if needed
5. Update README.md with new feature documentation

### Adding Memory Commands
1. Add variant to `MemoryCommand` enum in `src/cli.rs`
2. Add match arm in `execute_memory_command()` in `src/commands.rs`
3. Implement logic following existing patterns
4. Add help text in CLI enum
5. Test with `cargo run -- memory <command> --help`

### Code Quality Standards
- **Zero clippy warnings**: All code must pass `cargo clippy` without warnings
- **Minimal dependencies**: Reuse existing dependencies before adding new ones
- **Clone trait**: Add `#[derive(Clone)]` to structs that need to be shared across async contexts
- **Error handling**: Use proper `Result<T>` types and meaningful error messages

## Performance Guidelines

### Memory Management
- **Progressive file counting** during indexing
- **Preload file metadata** in HashMap for O(1) lookup
- **Smart merging** of single-line declarations
- **Context-aware markdown chunking** for better semantic search
- **Batch operations** for inserts/updates
- **Regular flush cycles** for persistence

### Database Efficiency
- **Use `content_exists()`** before processing
- **Batch operations** for inserts/updates
- **Regular flush cycles** for persistence
- **Differential processing** for file changes

## Common Patterns

### Error Handling
```rust
// ✅ GOOD: Proper error handling
pub async fn execute(config: &Config, command: Commands) -> Result<()> {
    match command {
        Commands::Memory { command } => {
            let mut memory_manager = MemoryManager::new(config).await?;
            execute_memory_command(&mut memory_manager, command).await
        }
        Commands::Mcp => {
            let working_directory = std::env::current_dir()?;
            let server = McpServer::new(config.clone(), working_directory).await?;
            server.run().await?;
        }
    }
}

// ❌ BAD: Unnecessary unwrap
let memory_manager = MemoryManager::new(config).await.unwrap(); // Don't do this
```

### Async Patterns
```rust
// ✅ GOOD: Proper async/await usage
let results = memory_manager.remember(&query, Some(filters)).await?;

// ❌ BAD: Blocking operations in async context
std::thread::sleep(std::time::Duration::from_secs(1)); // Don't do this
```

### CLI Usage Examples

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

# Start MCP server
octobrain mcp
```

## License

Apache-2.0

## Credits

Developed by Muvon Un Limited.
