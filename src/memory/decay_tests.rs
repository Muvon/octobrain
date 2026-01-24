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
    use super::super::types::{HybridSearchQuery, Memory, MemoryDecay, MemoryMetadata, MemoryType};
    use chrono::{Duration, Utc};

    #[test]
    fn test_memory_decay_creation() {
        let decay = MemoryDecay::new(0.8);

        assert_eq!(decay.base_importance, 0.8);
        assert_eq!(decay.access_count, 0);
        assert_eq!(decay.decay_rate, 1.0);

        // Last accessed should be recent (within last second)
        let now = Utc::now();
        let diff = (now - decay.last_accessed).num_seconds().abs();
        assert!(diff < 2, "Last accessed should be recent");
    }

    #[test]
    fn test_importance_decay_over_time() {
        let mut decay = MemoryDecay::new(1.0);

        // Simulate 30 days ago
        decay.last_accessed = Utc::now() - Duration::days(30);

        let current_importance = decay.calculate_current_importance(0.05);

        // After 30 days with decay_rate=1.0, importance should be ~0.368 (e^-1)
        // But with access_boost of ln(1) = 0, it becomes 0
        // So we need at least 1 access for meaningful importance
        assert!(
            current_importance >= 0.05,
            "Should respect minimum threshold"
        );
        assert!(current_importance < 1.0, "Should decay from original");
    }

    #[test]
    fn test_access_reinforcement() {
        let mut decay = MemoryDecay::new(0.5);

        // Simulate 60 days ago
        decay.last_accessed = Utc::now() - Duration::days(60);

        // Calculate importance with no accesses
        let importance_no_access = decay.calculate_current_importance(0.05);

        // Add multiple accesses
        for _ in 0..10 {
            decay.record_access();
        }

        // Calculate importance with accesses
        let importance_with_access = decay.calculate_current_importance(0.05);

        // With accesses, importance should be higher
        assert!(
            importance_with_access > importance_no_access,
            "Access should boost importance: {} vs {}",
            importance_with_access,
            importance_no_access
        );
    }

    #[test]
    fn test_record_access_updates_timestamp() {
        let mut decay = MemoryDecay::new(0.7);

        // Set last accessed to 10 days ago
        let old_time = Utc::now() - Duration::days(10);
        decay.last_accessed = old_time;
        decay.access_count = 0;

        // Record access
        decay.record_access();

        // Check access count increased
        assert_eq!(decay.access_count, 1);

        // Check timestamp updated (should be within last second)
        let now = Utc::now();
        let diff = (now - decay.last_accessed).num_seconds().abs();
        assert!(diff < 2, "Last accessed should be updated to now");
    }

    #[test]
    fn test_importance_floor() {
        let mut decay = MemoryDecay::new(0.1);

        // Simulate very old memory (1 year)
        decay.last_accessed = Utc::now() - Duration::days(365);

        let min_threshold = 0.05;
        let current_importance = decay.calculate_current_importance(min_threshold);

        // Should never go below minimum threshold
        assert!(
            current_importance >= min_threshold,
            "Importance should not go below floor: {}",
            current_importance
        );
    }

    #[test]
    fn test_update_base_importance() {
        let mut decay = MemoryDecay::new(0.5);

        decay.update_base_importance(0.9);
        assert_eq!(decay.base_importance, 0.9);

        // Test clamping to [0.0, 1.0]
        decay.update_base_importance(1.5);
        assert_eq!(decay.base_importance, 1.0);

        decay.update_base_importance(-0.5);
        assert_eq!(decay.base_importance, 0.0);
    }

    #[test]
    fn test_memory_get_current_importance_with_decay_enabled() {
        let mut metadata = MemoryMetadata::default();
        metadata.importance = 0.8;
        metadata.decay = MemoryDecay::new(0.8);

        // Simulate 30 days old
        metadata.decay.last_accessed = Utc::now() - Duration::days(30);
        metadata.decay.access_count = 5;

        let memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            Some(metadata),
        );

        // With decay enabled
        let importance_with_decay = memory.get_current_importance(true, 0.05);

        // Without decay
        let importance_without_decay = memory.get_current_importance(false, 0.05);

        // Without decay should return base importance
        assert_eq!(importance_without_decay, 0.8);

        // With decay should be different (likely lower due to time)
        assert!(importance_with_decay <= 0.8);
    }

    #[test]
    fn test_memory_record_access() {
        let mut memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );

        let initial_count = memory.metadata.decay.access_count;

        memory.record_access();

        assert_eq!(memory.metadata.decay.access_count, initial_count + 1);
    }

    #[test]
    fn test_decay_formula_correctness() {
        let mut decay = MemoryDecay::new(1.0);
        decay.decay_rate = 1.0;
        decay.access_count = 0;

        // Test at different time points

        // Day 0 (just created)
        decay.last_accessed = Utc::now();
        let importance_day_0 = decay.calculate_current_importance(0.0);
        // ln(1) = 0, so importance should be 0 or min_threshold

        // Day 30
        decay.last_accessed = Utc::now() - Duration::days(30);
        decay.access_count = 1; // Need at least 1 access for non-zero importance
        let importance_day_30 = decay.calculate_current_importance(0.0);

        // Day 60
        decay.last_accessed = Utc::now() - Duration::days(60);
        decay.access_count = 1;
        let importance_day_60 = decay.calculate_current_importance(0.0);

        // Importance should decrease over time
        assert!(
            importance_day_30 > importance_day_60,
            "Importance should decay over time: day30={} vs day60={}",
            importance_day_30,
            importance_day_60
        );
    }

    #[test]
    fn test_multiple_accesses_compound_boost() {
        let mut decay = MemoryDecay::new(0.5);
        decay.last_accessed = Utc::now() - Duration::days(30);

        // Test with increasing access counts
        let mut importances = Vec::new();

        for count in [1, 5, 10, 20, 50] {
            decay.access_count = count;
            let importance = decay.calculate_current_importance(0.0);
            importances.push(importance);
        }

        // Each subsequent importance should be higher (logarithmic growth)
        for i in 1..importances.len() {
            assert!(
                importances[i] > importances[i - 1],
                "More accesses should increase importance: {} vs {}",
                importances[i],
                importances[i - 1]
            );
        }
    }

    #[test]
    fn test_decay_disabled_returns_base_importance() {
        let mut metadata = MemoryMetadata::default();
        metadata.importance = 0.75;
        metadata.decay = MemoryDecay::new(0.75);
        metadata.decay.last_accessed = Utc::now() - Duration::days(365); // Very old

        let memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            Some(metadata),
        );

        // With decay disabled, should always return base importance
        let importance = memory.get_current_importance(false, 0.05);
        assert_eq!(importance, 0.75);
    }

    // Hybrid Search Query Tests

    #[test]
    fn test_hybrid_query_default_weights() {
        let query = HybridSearchQuery::default();

        assert_eq!(query.vector_weight, 0.6);
        assert_eq!(query.keyword_weight, 0.2);
        assert_eq!(query.recency_weight, 0.1);
        assert_eq!(query.importance_weight, 0.1);

        // Weights should sum to 1.0
        let sum = query.vector_weight
            + query.keyword_weight
            + query.recency_weight
            + query.importance_weight;
        assert!(
            (sum - 1.0).abs() < 0.001,
            "Weights should sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_weight_normalization() {
        let mut query = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            keywords: None,
            vector_weight: 2.0,
            keyword_weight: 1.0,
            recency_weight: 1.0,
            importance_weight: 0.0,
            filters: Default::default(),
        };

        query.normalize_weights();

        // After normalization, weights should sum to 1.0
        let sum = query.vector_weight
            + query.keyword_weight
            + query.recency_weight
            + query.importance_weight;
        assert!(
            (sum - 1.0).abs() < 0.001,
            "Normalized weights should sum to 1.0, got {}",
            sum
        );

        // Check proportions are maintained
        assert!((query.vector_weight - 0.5).abs() < 0.001); // 2/4 = 0.5
        assert!((query.keyword_weight - 0.25).abs() < 0.001); // 1/4 = 0.25
        assert!((query.recency_weight - 0.25).abs() < 0.001); // 1/4 = 0.25
        assert_eq!(query.importance_weight, 0.0);
    }

    #[test]
    fn test_weight_validation() {
        // Valid query
        let valid_query = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            keywords: None,
            vector_weight: 0.5,
            keyword_weight: 0.3,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(valid_query.validate().is_ok());

        // Invalid: weight > 1.0
        let invalid_query = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            keywords: None,
            vector_weight: 1.5,
            keyword_weight: 0.2,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query.validate().is_err());

        // Invalid: weight < 0.0
        let invalid_query2 = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            keywords: None,
            vector_weight: 0.5,
            keyword_weight: -0.1,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query2.validate().is_err());

        // Invalid: no query or keywords
        let invalid_query3 = HybridSearchQuery {
            vector_query: None,
            keywords: None,
            vector_weight: 0.5,
            keyword_weight: 0.3,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query3.validate().is_err());
    }
}
