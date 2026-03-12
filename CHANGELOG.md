# Changelog

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
