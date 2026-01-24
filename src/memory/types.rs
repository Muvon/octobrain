// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Types of memories that can be stored - unified for comprehensive coverage
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemoryType {
    /// Code insights, patterns, solutions, and implementations
    Code,
    /// System architecture, design decisions, and patterns
    Architecture,
    /// Bug fixes, issues, and troubleshooting solutions
    BugFix,
    /// Feature implementations, requirements, and specifications
    Feature,
    /// Documentation, explanations, and knowledge
    Documentation,
    /// User preferences, settings, and workflow patterns
    UserPreference,
    /// Project decisions, meeting notes, and planning
    Decision,
    /// Learning notes, tutorials, and educational content
    Learning,
    /// Configuration, environment setup, and deployment
    Configuration,
    /// Testing strategies, test cases, and QA insights
    Testing,
    /// Performance optimizations and monitoring insights
    Performance,
    /// Security considerations, vulnerabilities, and fixes
    Security,
    /// General insights, tips, and miscellaneous knowledge
    Insight,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::Code => write!(f, "code"),
            MemoryType::Architecture => write!(f, "architecture"),
            MemoryType::BugFix => write!(f, "bug_fix"),
            MemoryType::Feature => write!(f, "feature"),
            MemoryType::Documentation => write!(f, "documentation"),
            MemoryType::UserPreference => write!(f, "user_preference"),
            MemoryType::Decision => write!(f, "decision"),
            MemoryType::Learning => write!(f, "learning"),
            MemoryType::Configuration => write!(f, "configuration"),
            MemoryType::Testing => write!(f, "testing"),
            MemoryType::Performance => write!(f, "performance"),
            MemoryType::Security => write!(f, "security"),
            MemoryType::Insight => write!(f, "insight"),
        }
    }
}

impl From<String> for MemoryType {
    fn from(s: String) -> Self {
        match s.to_lowercase().as_str() {
            "code" => MemoryType::Code,
            "architecture" => MemoryType::Architecture,
            "bug_fix" | "bugfix" | "bug" => MemoryType::BugFix,
            "feature" => MemoryType::Feature,
            "documentation" | "docs" | "doc" => MemoryType::Documentation,
            "user_preference" | "preference" | "user" => MemoryType::UserPreference,
            "decision" | "meeting" | "planning" => MemoryType::Decision,
            "learning" | "tutorial" | "education" => MemoryType::Learning,
            "configuration" | "config" | "setup" | "deployment" => MemoryType::Configuration,
            "testing" | "test" | "qa" => MemoryType::Testing,
            "performance" | "perf" | "optimization" => MemoryType::Performance,
            "security" | "vulnerability" | "vuln" => MemoryType::Security,
            _ => MemoryType::Insight, // Default fallback
        }
    }
}

/// Temporal decay tracking for memory importance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDecay {
    /// Base importance score before decay (0.0 to 1.0)
    pub base_importance: f32,
    /// Number of times this memory has been accessed
    pub access_count: u32,
    /// Last time this memory was accessed
    pub last_accessed: DateTime<Utc>,
    /// Decay rate (higher = faster decay)
    pub decay_rate: f32,
}

impl MemoryDecay {
    /// Create new decay tracker with base importance
    pub fn new(base_importance: f32) -> Self {
        let now = Utc::now();
        Self {
            base_importance,
            access_count: 0,
            last_accessed: now,
            decay_rate: 1.0, // Default decay rate
        }
    }

    /// Calculate current importance based on temporal decay and access reinforcement
    /// Formula: importance = base_importance * exp(-decay_rate * days_since_access / 30) * ln(access_count + 1)
    pub fn calculate_current_importance(&self, min_threshold: f32) -> f32 {
        let now = Utc::now();
        let days_since_access = (now - self.last_accessed).num_days() as f32;

        // Exponential decay over time (normalized to 30-day periods)
        let time_decay = (-self.decay_rate * days_since_access / 30.0).exp();

        // Access reinforcement using logarithmic scaling
        let access_boost = (self.access_count as f32 + 1.0).ln();

        // Combined score with minimum threshold
        let current_importance = self.base_importance * time_decay * access_boost;
        current_importance.max(min_threshold)
    }

    /// Record an access to this memory
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_accessed = Utc::now();
    }

    /// Update base importance (e.g., when memory is manually updated)
    pub fn update_base_importance(&mut self, new_importance: f32) {
        self.base_importance = new_importance.clamp(0.0, 1.0);
    }
}

impl Default for MemoryDecay {
    fn default() -> Self {
        Self::new(0.5)
    }
}

/// Metadata associated with a memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// Git commit hash when memory was created
    pub git_commit: Option<String>,
    /// Files associated with this memory
    pub related_files: Vec<String>,
    /// Tags for categorization and search
    pub tags: Vec<String>,
    /// Importance score (0.0 to 1.0)
    pub importance: f32,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
    /// User who created the memory
    pub created_by: Option<String>,
    /// Additional key-value metadata
    pub custom_fields: HashMap<String, String>,
    /// Temporal decay tracking
    pub decay: MemoryDecay,
}

impl Default for MemoryMetadata {
    fn default() -> Self {
        Self {
            git_commit: None,
            related_files: Vec::new(),
            tags: Vec::new(),
            importance: 0.5,
            confidence: 1.0,
            created_by: None,
            custom_fields: HashMap::new(),
            decay: MemoryDecay::new(0.5),
        }
    }
}

/// Core memory structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Unique identifier
    pub id: String,
    /// Type of memory
    pub memory_type: MemoryType,
    /// Short summary/title
    pub title: String,
    /// Detailed content
    pub content: String,
    /// Associated metadata
    pub metadata: MemoryMetadata,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
    /// Optional relevance score from search (not stored)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,
}

impl Memory {
    /// Create a new memory
    pub fn new(
        memory_type: MemoryType,
        title: String,
        content: String,
        metadata: Option<MemoryMetadata>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            memory_type,
            title,
            content,
            metadata: metadata.unwrap_or_default(),
            created_at: now,
            updated_at: now,
            relevance_score: None,
        }
    }

    /// Update the memory content and metadata
    pub fn update(
        &mut self,
        title: Option<String>,
        content: Option<String>,
        metadata: Option<MemoryMetadata>,
    ) {
        if let Some(title) = title {
            self.title = title;
        }
        if let Some(content) = content {
            self.content = content;
        }
        if let Some(metadata) = metadata {
            self.metadata = metadata;
        }
        self.updated_at = Utc::now();
    }

    /// Get searchable text for embedding generation
    pub fn get_searchable_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.title,
            self.content,
            self.metadata.tags.join(" "),
            self.metadata.related_files.join(" ")
        )
    }

    /// Get current importance considering temporal decay
    pub fn get_current_importance(&self, decay_enabled: bool, min_threshold: f32) -> f32 {
        if decay_enabled {
            self.metadata
                .decay
                .calculate_current_importance(min_threshold)
        } else {
            self.metadata.importance
        }
    }

    /// Record access to this memory (for decay reinforcement)
    pub fn record_access(&mut self) {
        self.metadata.decay.record_access();
    }

    /// Add a tag if it doesn't exist
    pub fn add_tag(&mut self, tag: String) {
        if !self.metadata.tags.contains(&tag) {
            self.metadata.tags.push(tag);
            self.updated_at = Utc::now();
        }
    }

    /// Remove a tag
    pub fn remove_tag(&mut self, tag: &str) {
        if let Some(pos) = self.metadata.tags.iter().position(|t| t == tag) {
            self.metadata.tags.remove(pos);
            self.updated_at = Utc::now();
        }
    }

    /// Add a related file if it doesn't exist
    pub fn add_related_file(&mut self, file_path: String) {
        if !self.metadata.related_files.contains(&file_path) {
            self.metadata.related_files.push(file_path);
            self.updated_at = Utc::now();
        }
    }

    /// Remove a related file
    pub fn remove_related_file(&mut self, file_path: &str) {
        if let Some(pos) = self
            .metadata
            .related_files
            .iter()
            .position(|f| f == file_path)
        {
            self.metadata.related_files.remove(pos);
            self.updated_at = Utc::now();
        }
    }
}

/// Query parameters for memory search
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// Text query for semantic search
    pub query_text: Option<String>,
    /// Filter by memory types
    pub memory_types: Option<Vec<MemoryType>>,
    /// Filter by tags (any of these tags)
    pub tags: Option<Vec<String>>,
    /// Filter by related files
    pub related_files: Option<Vec<String>>,
    /// Filter by git commit
    pub git_commit: Option<String>,
    /// Filter by minimum importance score
    pub min_importance: Option<f32>,
    /// Filter by minimum confidence score
    pub min_confidence: Option<f32>,
    /// Filter by creation date range
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    /// Maximum number of results
    pub limit: Option<usize>,
    /// Minimum relevance score for vector search
    pub min_relevance: Option<f32>,
    /// Sort by field
    pub sort_by: Option<MemorySortBy>,
    /// Sort order
    pub sort_order: Option<SortOrder>,
}

/// Hybrid search query combining multiple retrieval signals
#[derive(Debug, Clone)]
pub struct HybridSearchQuery {
    /// Vector semantic search query
    pub vector_query: Option<String>,
    /// Keywords for exact/fuzzy matching
    pub keywords: Option<Vec<String>>,
    /// Weight for vector similarity signal (0.0-1.0)
    pub vector_weight: f32,
    /// Weight for keyword matching signal (0.0-1.0)
    pub keyword_weight: f32,
    /// Weight for recency signal (0.0-1.0)
    pub recency_weight: f32,
    /// Weight for importance signal (0.0-1.0)
    pub importance_weight: f32,
    /// Standard filters (same as MemoryQuery)
    pub filters: MemoryQuery,
}

impl Default for HybridSearchQuery {
    fn default() -> Self {
        Self {
            vector_query: None,
            keywords: None,
            vector_weight: 0.6,
            keyword_weight: 0.2,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: MemoryQuery::default(),
        }
    }
}

impl HybridSearchQuery {
    /// Normalize weights to sum to 1.0
    pub fn normalize_weights(&mut self) {
        let sum =
            self.vector_weight + self.keyword_weight + self.recency_weight + self.importance_weight;
        if sum > 0.0 {
            self.vector_weight /= sum;
            self.keyword_weight /= sum;
            self.recency_weight /= sum;
            self.importance_weight /= sum;
        }
    }

    /// Validate that weights are in valid ranges
    pub fn validate(&self) -> Result<(), String> {
        if self.vector_weight < 0.0 || self.vector_weight > 1.0 {
            return Err(format!(
                "vector_weight must be in [0.0, 1.0], got {}",
                self.vector_weight
            ));
        }
        if self.keyword_weight < 0.0 || self.keyword_weight > 1.0 {
            return Err(format!(
                "keyword_weight must be in [0.0, 1.0], got {}",
                self.keyword_weight
            ));
        }
        if self.recency_weight < 0.0 || self.recency_weight > 1.0 {
            return Err(format!(
                "recency_weight must be in [0.0, 1.0], got {}",
                self.recency_weight
            ));
        }
        if self.importance_weight < 0.0 || self.importance_weight > 1.0 {
            return Err(format!(
                "importance_weight must be in [0.0, 1.0], got {}",
                self.importance_weight
            ));
        }

        // Check if at least one signal is enabled
        if self.vector_query.is_none() && self.keywords.is_none() {
            return Err("At least one of vector_query or keywords must be provided".to_string());
        }

        Ok(())
    }
}

/// Keyword match information for debugging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordMatch {
    /// The matched keyword
    pub keyword: String,
    /// Number of occurrences
    pub count: usize,
    /// Locations where found (title, content, tags)
    pub locations: Vec<String>,
}

/// Search signal contribution for debugging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchSignal {
    /// Vector similarity score
    Vector(f32),
    /// Keyword matching score
    Keyword(f32),
    /// Recency score
    Recency(f32),
    /// Importance score
    Importance(f32),
}

/// Sort options for memory queries
#[derive(Debug, Clone)]
pub enum MemorySortBy {
    CreatedAt,
    Importance,
}

/// Sort order
#[derive(Debug, Clone)]
#[allow(dead_code)] // Ascending is used in match statement but compiler doesn't detect it
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Search result with relevance scoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResult {
    /// The memory
    pub memory: Memory,
    /// Relevance score from vector search
    pub relevance_score: f32,
    /// Explanation of why this memory was selected
    pub selection_reason: String,
}

/// Memory relationship between memories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRelationship {
    /// Unique identifier
    pub id: String,
    /// Source memory ID
    pub source_id: String,
    /// Target memory ID
    pub target_id: String,
    /// Type of relationship
    pub relationship_type: RelationshipType,
    /// Strength of relationship (0.0 to 1.0)
    pub strength: f32,
    /// Description of the relationship
    pub description: String,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Types of relationships between memories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationshipType {
    /// One memory relates to another
    RelatedTo,
    /// One memory depends on another
    DependsOn,
    /// One memory supersedes another
    Supersedes,
    /// Memories are similar or duplicate
    Similar,
    /// Memories conflict with each other
    Conflicts,
    /// One memory implements another
    Implements,
    /// One memory extends another
    Extends,
    /// Custom relationship type
    Custom(String),
}

impl std::fmt::Display for RelationshipType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationshipType::RelatedTo => write!(f, "related_to"),
            RelationshipType::DependsOn => write!(f, "depends_on"),
            RelationshipType::Supersedes => write!(f, "supersedes"),
            RelationshipType::Similar => write!(f, "similar"),
            RelationshipType::Conflicts => write!(f, "conflicts"),
            RelationshipType::Implements => write!(f, "implements"),
            RelationshipType::Extends => write!(f, "extends"),
            RelationshipType::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// Configuration for memory system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum number of memories to keep
    pub max_memories: Option<usize>,
    /// Automatic cleanup threshold (days)
    pub auto_cleanup_days: Option<u32>,
    /// Minimum importance for automatic cleanup
    pub cleanup_min_importance: f32,
    /// Enable automatic relationship detection
    pub auto_relationships: bool,
    /// Relationship detection threshold
    pub relationship_threshold: f32,
    /// Maximum memories returned in search
    pub max_search_results: usize,
    /// Default importance for new memories
    pub default_importance: f32,
    /// Enable temporal decay system
    pub decay_enabled: bool,
    /// Half-life for importance decay in days (time for importance to halve)
    pub decay_half_life_days: u32,
    /// Boost factor for access reinforcement (multiplier per access)
    pub access_boost_factor: f32,
    /// Minimum importance threshold (floor value after decay)
    pub min_importance_threshold: f32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_memories: Some(10000),
            auto_cleanup_days: Some(365),
            cleanup_min_importance: 0.1,
            auto_relationships: true,
            relationship_threshold: 0.7,
            max_search_results: 50,
            default_importance: 0.5,
            decay_enabled: true,
            decay_half_life_days: 90, // 3 months half-life
            access_boost_factor: 1.2,
            min_importance_threshold: 0.05, // 5% minimum
        }
    }
}
