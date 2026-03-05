use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents a chunk of knowledge content.
///
/// For parent-child chunking: `content` is the small child text used for embedding/search.
/// `parent_content` is the full section text returned to the user when present —
/// it gives richer context without polluting the embedding with too much noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeChunk {
    pub id: String,
    pub source_url: String,
    pub source_title: String,
    pub chunk_index: i32,
    /// Small child text — what gets embedded and matched against queries.
    pub content: String,
    /// Full parent section text — returned to the user instead of content when present.
    /// None when the section was already small enough to be its own child.
    pub parent_content: Option<String>,
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
