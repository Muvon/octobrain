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
        }
    }
}
