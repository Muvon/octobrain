use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents a chunk of knowledge content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub id: String,
    pub source_url: String,
    pub source_title: String,
    pub chunk_index: i32,
    pub content: String,
    pub section_path: Vec<String>,
    pub char_start: usize,
    pub char_end: usize,
}

/// Search result with relevance score
#[derive(Debug, Clone)]
pub struct KnowledgeSearchResult {
    pub chunk: KnowledgeChunk,
    pub relevance_score: f32,
}

/// Statistics about the knowledge base
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeStats {
    pub total_sources: usize,
    pub total_chunks: usize,
    pub oldest_indexed: Option<DateTime<Utc>>,
    pub newest_indexed: Option<DateTime<Utc>>,
}

/// Result of indexing operation
#[derive(Debug, Clone)]
pub struct IndexResult {
    pub url: String,
    pub chunks_created: usize,
    pub was_cached: bool,
    pub content_changed: bool,
}
