# Changelog

## [0.6.1] - 2026-05-14

### 📋 Release Summary

This release introduces a dedicated production release profile and updated dependencies to ensure optimal performance and stability (3063c17b). These configuration improvements streamline the build process and enhance the overall reliability of the memory management system.


### 🔄 Other Changes

1 maintenance, dependency, and tooling update not listed individually.

## [0.6.0] - 2026-05-03

### 📋 Release Summary

This release introduces advanced content retrieval capabilities, including regex pattern matching and full content extraction tools for indexed knowledge (b4c6e800, 0ca75b74). System reliability and performance are significantly improved through optimized memory streaming, stricter path validation, and enhanced vector dimension handling (95f4deb4, 87df649d, 6f831de7). Additionally, the update streamlines the toolset and provides expanded documentation for configuration and development (104c0b59, 069d0495, a44ef316).


### ✨ New Features & Enhancements

- **knowledge**: add regex pattern matching for indexed content `b4c6e800`
- **knowledge**: implement read command for full content extraction `0ca75b74`

### 🔧 Improvements & Optimizations

- **mcp**: remove auto_link tool `104c0b59`
- **knowledge**: stream query results to limit memory `95f4deb4`
- **knowledge**: simplify logic and improve safety `2963f38e`

### 🐛 Bug Fixes & Stability

- **knowledge**: use platform temp dir in tests `201d305f`
- **knowledge**: reject directory paths as sources `87df649d`
- **vector**: ensure sub-vectors divide dimension `6f831de7`
- **knowledge**: normalize source uris and match output `4b154ce1`

### 📚 Documentation & Examples

- **instructions**: rewrite development guide for clarity `a44ef316`
- **knowledge**: add read and match tool documentation `921c9d7e`
- **config**: expand memory and knowledge settings `069d0495`

### 🔄 Other Changes

3 maintenance, dependency, and tooling updates not listed individually.

## [0.5.1] - 2026-04-19

### 📋 Release Summary

This release improves system stability by refining memory cleanup logic to prevent errors when project keys are missing (595318eb). Additionally, core dependencies and internal workflows have been updated to enhance overall performance and security (353f6fc4, afe734ca).


### 🐛 Bug Fixes & Stability

- **memory**: skip cleanup without project key `595318eb`

### 🔄 Other Changes

2 maintenance, dependency, and tooling updates not listed individually.

## [0.5.0] - 2026-04-08

### 📋 Release Summary

This release introduces local file indexing capabilities to the knowledge system (breaking change), enabling direct interaction with local files. Memory management has been significantly enhanced with new product and workflow memory categories, session-scoped storage, and automatic stale reference cleanup. The MCP protocol implementation has been upgraded to the official rmcp SDK, along with various dependency updates and documentation improvements.


### 🚨 Breaking Changes

⚠️ **Important**: This release contains breaking changes that may require code updates.

- **knowledge**: support local file indexing `0ab5a13f`

### ✨ New Features & Enhancements

- **memory**: add stale file reference cleanup `46c8827d`
- **knowledge**: add session-scoped storage and commands `21216453`
- **memory**: add product and workflow memory type categories `04b145a1`
- **mcp**: replace custom server with official rmcp SDK `1f581537`

### 🔧 Improvements & Optimizations

- **memory**: add git-aware stale reference cleanup `be812684`
- **knowledge**: use std::slice::from_ref for single-element test slices `21f9da28`
- **legal**: apply copyright header standards `556b1228`
- **mcp**: use builder pattern for ServerInfo construction `8041b71a`
- **mcp**: rename rmcp_server to server and add typed schemas `56b3b6d9`

### 📚 Documentation & Examples

- restructure README for clearer onboarding `78918bd1`

### 🔄 Other Changes

5 maintenance, dependency, and tooling updates not listed individually.

## [0.4.2] - 2026-03-21

### 📋 Release Summary

This release updates internal dependencies and upgrades the Rust toolchain to version 1.94.0, ensuring improved performance and compatibility.


### 🔄 Other Changes

- **deps**: update Cargo.lock `5b758fa8`
- upgrade Rust toolchain to 1.94.0 `c6c56997`

### 📊 Release Summary

**Total commits**: 2 across 1 categories

🔄 **2** other changes - *Maintenance & tooling*

## [0.4.1] - 2026-03-15

### 📋 Release Summary

This release streamlines cross-project memory queries by making project keys optional, allowing easier access to shared context across workspaces (4ccc1cb0). Additionally, automated Homebrew tap notifications ensure faster availability of future updates for macOS users (cae97de5).


### 🔧 Improvements & Optimizations

- **memory**: make project_key optional for cross-project queries `4ccc1cb0`

### 🔄 Other Changes

- **release**: add homebrew tap notification job `cae97de5`

### 📊 Release Summary

**Total commits**: 2 across 2 categories

🔧 **1** improvement - *Better performance & code quality*
🔄 **1** other change - *Maintenance & tooling*

## [0.4.0] - 2026-03-14

### 📋 Release Summary

This release introduces configurable knowledge and memory settings alongside automatic index optimization for improved performance (6d083380, d3d4c3e5). Session management is enhanced with locked project and role assignments during initialization (4ca28abd), while memory operations are streamlined for better efficiency and clarity (06c352fb, 7008ec38, 039c8f9c).


### ✨ New Features & Enhancements

- **config**: add knowledge and memory configuration `6d083380`
- **mcp**: lock session project and role on initialize `4ca28abd`
- **vector**: add automatic index optimization `d3d4c3e5`

### 🔧 Improvements & Optimizations

- **memory**: atomic upsert with merge_insert `06c352fb`
- **mcp**: remove token truncation from memory tools `7008ec38`
- **mcp**: simplify memory tool descriptions `039a8f9c`

### 🔄 Other Changes

- **git**: ignore mcpregistry temporary files `399431ef`

### 📊 Release Summary

**Total commits**: 7 across 3 categories

✨ **3** new features - *Enhanced functionality*
🔧 **3** improvements - *Better performance & code quality*
🔄 **1** other change - *Maintenance & tooling*

## [0.3.0] - 2026-03-13

### 📋 Release Summary

This release introduces confidence scoring and relationship tools for smarter memory management, plus enhanced search with parent content tracking and reranker support (ab2d791b, 502cbe62, 98c7e2f8). The MCP server gains auto-link and memory-graph capabilities with optional HTTP transport (f201c7d4, d7787b76). Multiple fixes improve search accuracy, data isolation, and cross-platform stability (b1524de0, 84dc6854, 0b1df7ee).


### ✨ New Features & Enhancements

- **memory**: add confidence scoring and relationship tools `ab2d791b`
- **mcp**: add auto_link and memory_graph tools with enhanced schema `f201c7d4`
- **mcp**: add HTTP transport option `d7787b76`
- **knowledge**: add parent content tracking for search context `502cbe62`
- **memory**: activate reranker for hybrid and vector search `98c7e2f8`

### 🔧 Improvements & Optimizations

- **storage**: consolidate to shared memory database `3e0b1b8d`
- **hybrid**: switch to native lancedb rrf fusion `df9497d7`
- **knowledge**: improve HTML extraction with readability `090670c1`

### 🐛 Bug Fixes & Stability

- **server**: align MCP server manifest with latest schema `b1524de0`
- **server**: shorten description in server manifest `f3ff987d`
- **memory**: add missing id field to relationships table `54b426c1`
- **memory**: scope delete operations to project key `84dc6854`
- **memory**: remove keyword search from hybrid query `8e68099b`
- **ci**: enable default features and fix Windows static CRT build `0b1df7ee`

### 🔄 Other Changes

- **deps**: upgrade html parsing and text extraction libraries `19352773`
- **deps**: bump octolib to 0.10.4 `80de4030`
- **release**: fix musl builds by disabling default features `a37aacb6`
- **release**: 0.3.0 `bb3f9b98`
- **deps**: upgrade datafusion to 51.0.0 and arrow to 57.3.0 `ec4f322e`
- disable fastembed on Windows builds `1b263d44`
- **ci**: install protoc using setup-protoc action `8cac4795`
- **chunker**: fix extract_title method name in tests `f8664956`
- **deps**: upgrade octolib to 0.10.0 `c86805f1`
- **deps**: bump octolib to 0.9.3 `d24505c8`

### 📊 Release Summary

**Total commits**: 24 across 4 categories

✨ **5** new features - *Enhanced functionality*
🔧 **3** improvements - *Better performance & code quality*
🐛 **6** bug fixes - *Improved stability*
🔄 **10** other changes - *Maintenance & tooling*

## [0.3.0] - 2026-03-12

### 📋 Release Summary

This release introduces enhanced memory management with auto-linking capabilities and improved search functionality through hybrid vector search with reranking (f201c7d4, 98c7e2f8, 502cbe62). The system now supports HTTP transport for broader integration options and features consolidated database architecture for improved performance (d7787b76, 3e0b1b8d). Multiple bug fixes enhance data integrity, project scoping, and cross-platform compatibility (54b426c1, 84dc6854, 0b1df7ee).


### ✨ New Features & Enhancements

- **mcp**: add auto_link and memory_graph tools with enhanced schema `f201c7d4`
- **mcp**: add HTTP transport option `d7787b76`
- **knowledge**: add parent content tracking for search context `502cbe62`
- **memory**: activate reranker for hybrid and vector search `98c7e2f8`

### 🔧 Improvements & Optimizations

- **storage**: consolidate to shared memory database `3e0b1b8d`
- **hybrid**: switch to native lancedb rrf fusion `df9497d7`
- **knowledge**: improve HTML extraction with readability `090670c1`

### 🐛 Bug Fixes & Stability

- **memory**: add missing id field to relationships table `54b426c1`
- **memory**: scope delete operations to project key `84dc6854`
- **memory**: remove keyword search from hybrid query `8e68099b`
- **ci**: enable default features and fix Windows static CRT build `0b1df7ee`

### 🔄 Other Changes

- **deps**: upgrade datafusion to 51.0.0 and arrow to 57.3.0 `ec4f322e`
- disable fastembed on Windows builds `1b263d44`
- **ci**: install protoc using setup-protoc action `8cac4795`
- **chunker**: fix extract_title method name in tests `f8664956`
- **deps**: upgrade octolib to 0.10.0 `c86805f1`
- **deps**: bump octolib to 0.9.3 `d24505c8`

### 📊 Release Summary

**Total commits**: 17 across 4 categories

✨ **4** new features - *Enhanced functionality*
🔧 **3** improvements - *Better performance & code quality*
🐛 **4** bug fixes - *Improved stability*
🔄 **6** other changes - *Maintenance & tooling*

## [0.2.0] - 2026-02-16

### 📋 Release Summary

This release introduces hybrid BM25 search for more accurate knowledge retrieval and adds live URL indexing for real-time content extraction (0dda1e5e, f10cfea5). Performance and stability improvements include optimized search relevance scoring, refined knowledge base defaults, and updated core dependencies (b04079e8, f7f8b22d, 1771be34, 754e8ec2).


### ✨ New Features & Enhancements

- **knowledge**: add bm25 hybrid search to knowledge retrieval `0dda1e5e`
- **knowledge**: add URL indexing with live extraction `f10cfea5`

### 🔧 Improvements & Optimizations

- **octolib**: upgrade to 0.8.1 and update embedding interface `6a650b6f`

### 🐛 Bug Fixes & Stability

- **knowledge**: resolve hybrid search relevance scoring and crash `b04079e8`
- **config**: adjust knowledge base defaults for better performance `f7f8b22d`

### 🔄 Other Changes

- **deps**: bump octolib to 0.9.0 `1771be34`
- **deps**: update tokio chrono uuid clap versions `754e8ec2`

### 📊 Release Summary

**Total commits**: 7 across 4 categories

✨ **2** new features - *Enhanced functionality*
🔧 **1** improvement - *Better performance & code quality*
🐛 **2** bug fixes - *Improved stability*
🔄 **2** other changes - *Maintenance & tooling*

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-02-04

### 📋 Release Summary

This release introduces hybrid search with reranking support, auto-linking functionality via MemoryGraph, and temporal decay for memory importance management. Enhanced cross-platform build support and improved logging with rotation provide better reliability and observability.


### ✨ New Features & Enhancements

- **search**: add reranking support `24245634`
- **build**: expand cross-platform builds and embedding providers `a8aab771`
- **mcp**: add structured file logging with rotation `14f79109`
- **autolink**: add auto-linking functionality and MemoryGraph `9181de27`
- **search**: implement hybrid search combining multiple signals `edfbca67`
- **memory**: add temporal decay for memory importance `4f38123b`
- **shell**: add completion install script and Makefile `f5ab4beb`

### 🔧 Improvements & Optimizations

- **mcp**: enable lazy memory provider initialization `4414400c`
- **memory**: configuration options and code cleanup `6f0190ea`
- **memory**: extract to standalone octobrain project `ea7b47e6`

### 🐛 Bug Fixes & Stability

- **memory**: validate empty text and query before processing `cc91a956`

### 🔄 Other Changes

- simplify pre-commit config and normalize file formatting `91221bda`
- update dependencies and fix doc example imports `c72f508d`
- update octolib dependency to 0.7.0 `2917e6ae`
- optimize cache keys for feature matrix `e877b204`
- **memory**: use struct init and range contains in assertions `7a70f6db`
- **deps**: upgrade lance to v1.0 `301fabdd`
- add release workflow for semver tags `12a2a862`

### 📊 Release Summary

**Total commits**: 18 across 4 categories

✨ **7** new features - *Enhanced functionality*
🔧 **3** improvements - *Better performance & code quality*
🐛 **1** bug fix - *Improved stability*
🔄 **7** other changes - *Maintenance & tooling*
