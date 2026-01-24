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

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "octobrain")]
#[command(version, author = "Muvon Un Limited <opensource@muvon.io>")]
#[command(about = "Standalone memory management system for AI context and conversation state", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Memory management for storing and retrieving information
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Start MCP server (Model Context Protocol) exposing memory tools
    Mcp,
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommand {
    /// Store important information, insights, or context in memory
    Memorize {
        /// Short, descriptive title for the memory (5-200 characters)
        #[arg(short, long)]
        title: String,

        /// Detailed content to remember
        #[arg(short, long)]
        content: String,

        /// Category of memory for better organization
        #[arg(short = 'm', long, default_value = "code")]
        memory_type: String,

        /// Importance score from 0.0 to 1.0 (higher = more important)
        #[arg(short, long)]
        importance: Option<f32>,

        /// Tags for categorization (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Related file paths (comma-separated)
        #[arg(long)]
        files: Option<String>,
    },

    /// Search and retrieve stored memories using semantic search
    Remember {
        /// What you want to remember or search for (multiple queries for comprehensive search)
        queries: Vec<String>,

        /// Filter by memory types (comma-separated)
        #[arg(short = 'm', long)]
        memory_types: Option<String>,

        /// Filter by tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Filter by related files (comma-separated)
        #[arg(long)]
        files: Option<String>,

        /// Maximum number of memories to return
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Minimum relevance score (0.0-1.0)
        #[arg(long)]
        min_relevance: Option<f32>,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Permanently remove specific memories
    Forget {
        /// Specific memory ID to forget (get from remember results)
        #[arg(short, long)]
        memory_id: Option<String>,

        /// Query to find memories to forget (alternative to memory_id)
        #[arg(short, long)]
        query: Option<String>,

        /// Filter by memory types when using query (comma-separated)
        #[arg(short = 'm', long)]
        memory_types: Option<String>,

        /// Filter by tags when using query (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Confirm deletion without prompting
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Update an existing memory
    Update {
        /// Memory ID to update
        memory_id: String,

        /// New title (optional)
        #[arg(short, long)]
        title: Option<String>,

        /// New content (optional)
        #[arg(short, long)]
        content: Option<String>,

        /// New importance score (optional)
        #[arg(short, long)]
        importance: Option<f32>,

        /// Add tags (comma-separated)
        #[arg(long)]
        add_tags: Option<String>,

        /// Remove tags (comma-separated)
        #[arg(long)]
        remove_tags: Option<String>,

        /// Add related files (comma-separated)
        #[arg(long)]
        add_files: Option<String>,

        /// Remove related files (comma-separated)
        #[arg(long)]
        remove_files: Option<String>,
    },

    /// Get memory by ID
    Get {
        /// Memory ID to retrieve
        memory_id: String,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// List recent memories
    Recent {
        /// Maximum number of memories to show
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Filter by memory type
        #[arg(short = 'm', long)]
        memory_type: Option<String>,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "compact")]
        format: String,
    },

    /// Get memories by type
    ByType {
        /// Memory type to filter by
        memory_type: String,

        /// Maximum number of memories to show
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "compact")]
        format: String,
    },

    /// Get memories related to files
    ForFiles {
        /// File paths to search for (comma-separated)
        files: String,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Get memories by tags
    ByTags {
        /// Tags to search for (comma-separated)
        tags: String,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Get memories for current Git commit
    CurrentCommit {
        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show memory statistics
    Stats,

    /// Clean up old memories
    Cleanup {
        /// Confirm cleanup without prompting
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Clear ALL memory data (DANGEROUS: deletes everything)
    ClearAll {
        /// Confirm deletion without prompting
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Create a relationship between two memories
    Relate {
        /// Source memory ID
        source_id: String,

        /// Target memory ID
        target_id: String,

        /// Relationship type
        #[arg(short = 't', long, default_value = "related_to")]
        relationship_type: String,

        /// Relationship strength (0.0-1.0)
        #[arg(short, long, default_value = "0.5")]
        strength: f32,

        /// Description of relationship
        #[arg(short, long)]
        description: String,
    },

    /// Get relationships for a memory
    Relationships {
        /// Memory ID to get relationships for
        memory_id: String,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Get related memories through relationships
    Related {
        /// Memory ID to find related memories for
        memory_id: String,

        /// Output format: text, json, or compact
        #[arg(short, long, default_value = "text")]
        format: String,
    },
}
