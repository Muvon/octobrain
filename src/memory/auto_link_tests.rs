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

#[cfg(test)]
mod tests {
    use super::super::types::{MemoryConfig, MemoryGraph, RelationshipType};
    use std::collections::HashMap;

    #[test]
    fn test_auto_link_config_defaults() {
        let config = MemoryConfig::default();

        assert!(
            config.auto_linking_enabled,
            "Auto-linking should be enabled by default"
        );
        assert_eq!(
            config.auto_link_threshold, 0.78,
            "Default threshold should be 0.78"
        );
        assert_eq!(
            config.max_auto_links_per_memory, 5,
            "Default max links should be 5"
        );
        assert!(
            config.bidirectional_links,
            "Bidirectional links should be enabled by default"
        );
    }

    #[test]
    fn test_relationship_type_auto_linked() {
        use std::fmt::Write;

        let rel_type = RelationshipType::AutoLinked;
        let mut output = String::new();
        write!(&mut output, "{}", rel_type).unwrap();

        assert_eq!(output, "auto_linked");
    }

    #[test]
    fn test_memory_graph_creation() {
        let graph = MemoryGraph {
            root: "test-id".to_string(),
            memories: HashMap::new(),
            relationships: Vec::new(),
        };

        assert_eq!(graph.root, "test-id");
        assert!(graph.memories.is_empty());
        assert!(graph.relationships.is_empty());
    }

    #[test]
    fn test_auto_link_threshold_validation() {
        let config = MemoryConfig::default();

        // Threshold should be in reasonable range for quality links
        assert!(
            config.auto_link_threshold >= 0.7 && config.auto_link_threshold <= 0.9,
            "Threshold should be between 0.7 and 0.9 for quality links"
        );
    }

    #[test]
    fn test_max_auto_links_reasonable() {
        let config = MemoryConfig::default();

        // Max links should be reasonable to prevent over-linking
        assert!(
            config.max_auto_links_per_memory >= 3 && config.max_auto_links_per_memory <= 10,
            "Max links should be between 3 and 10"
        );
    }

    #[test]
    fn test_bidirectional_links_default() {
        let config = MemoryConfig::default();
        assert!(
            config.bidirectional_links,
            "Bidirectional links should be enabled for Zettelkasten-style linking"
        );
    }

    #[test]
    fn test_auto_link_config_customization() {
        let config = MemoryConfig {
            auto_linking_enabled: false,
            auto_link_threshold: 0.85,
            max_auto_links_per_memory: 3,
            bidirectional_links: false,
            ..Default::default()
        };

        assert!(!config.auto_linking_enabled);
        assert_eq!(config.auto_link_threshold, 0.85);
        assert_eq!(config.max_auto_links_per_memory, 3);
        assert!(!config.bidirectional_links);
    }

    #[test]
    fn test_memory_graph_with_data() {
        use super::super::types::{Memory, MemoryRelationship, MemoryType};
        use chrono::Utc;

        let mut graph = MemoryGraph {
            root: "root-id".to_string(),
            memories: HashMap::new(),
            relationships: Vec::new(),
        };

        // Add a memory
        let memory = Memory::new(
            MemoryType::Code,
            "Test Memory".to_string(),
            "Test content".to_string(),
            None,
        );
        graph.memories.insert(memory.id.clone(), memory);

        // Add a relationship
        let rel = MemoryRelationship {
            id: "rel-1".to_string(),
            source_id: "root-id".to_string(),
            target_id: "target-id".to_string(),
            relationship_type: RelationshipType::AutoLinked,
            strength: 0.85,
            description: "Auto-linked".to_string(),
            created_at: Utc::now(),
        };
        graph.relationships.push(rel);

        assert_eq!(graph.memories.len(), 1);
        assert_eq!(graph.relationships.len(), 1);
        assert_eq!(
            graph.relationships[0].relationship_type.to_string(),
            "auto_linked"
        );
    }
}
