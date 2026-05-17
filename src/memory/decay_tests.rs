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

    // Test fixtures: explicit values so the math in each test is easy to verify by hand.
    // 30-day half-life makes day-0/30/60 importance ratios exactly 1.0/0.5/0.25 for base=1.0.
    const HALF_LIFE_DAYS: u32 = 30;
    const BOOST_FACTOR: f32 = 1.2;

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

        let current_importance =
            decay.calculate_current_importance(1.0, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);

        // After 30 days at 30-day half-life with 0 accesses:
        //   1.0 * 0.5^(30/30) * (1 + 1.2*ln(1)) = 0.5 * 1.0 = 0.5
        assert!(
            (current_importance - 0.5).abs() < 0.01,
            "30-day-old memory at 30-day half-life should be ~0.5, got {}",
            current_importance
        );
        assert!(current_importance < 1.0, "Should decay from original");
    }

    #[test]
    fn test_access_reinforcement() {
        // Hold time_decay constant by pinning last_accessed; only access_count varies.
        let mut decay = MemoryDecay::new(0.5);
        decay.last_accessed = Utc::now() - Duration::days(60);

        let importance_no_access =
            decay.calculate_current_importance(0.5, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);

        decay.access_count = 10;
        let importance_with_access =
            decay.calculate_current_importance(0.5, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);

        assert!(
            importance_with_access > importance_no_access,
            "Access should boost importance: {} vs {}",
            importance_with_access,
            importance_no_access
        );
    }

    #[test]
    fn test_importance_floor() {
        let mut decay = MemoryDecay::new(0.1);

        // Simulate very old memory (1 year)
        decay.last_accessed = Utc::now() - Duration::days(365);

        let min_threshold = 0.05;
        let current_importance =
            decay.calculate_current_importance(0.1, min_threshold, HALF_LIFE_DAYS, BOOST_FACTOR);

        // Should never go below minimum threshold
        assert!(
            current_importance >= min_threshold,
            "Importance should not go below floor: {}",
            current_importance
        );
    }

    #[test]
    fn test_memory_get_current_importance_with_decay_enabled() {
        // 30 days old, ZERO accesses: pure half-life decay, no boost.
        // Half-life=30 → time_decay=0.5 → importance = 0.8 * 0.5 * 1.0 = 0.4
        let mut metadata = MemoryMetadata {
            importance: 0.8,
            decay: MemoryDecay::new(0.8),
            ..Default::default()
        };
        metadata.decay.last_accessed = Utc::now() - Duration::days(30);

        let memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            Some(metadata),
        );

        let importance_with_decay =
            memory.get_current_importance(true, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);
        let importance_without_decay =
            memory.get_current_importance(false, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);

        // Without decay returns raw base importance
        assert_eq!(importance_without_decay, 0.8);

        // With decay enabled, a 30-day-old unread memory must lose importance vs. its base
        assert!(
            importance_with_decay < importance_without_decay,
            "decay should reduce unread memory below base (got {} vs base {})",
            importance_with_decay,
            importance_without_decay
        );
        assert!(
            (importance_with_decay - 0.4).abs() < 0.01,
            "30-day-old unread memory at base=0.8, half-life=30 should be ~0.4, got {}",
            importance_with_decay
        );
    }

    #[test]
    fn test_per_memory_decay_rate_scales_half_life() {
        // decay_rate is a per-memory multiplier on the global half-life:
        //   effective_half_life = config_half_life / decay_rate
        // decay_rate=2.0 → memory decays twice as fast (importance lower at same age)
        // decay_rate=0.5 → memory decays twice as slow (importance higher at same age)
        let mut decay_fast = MemoryDecay::new(1.0);
        decay_fast.decay_rate = 2.0;
        decay_fast.last_accessed = Utc::now() - Duration::days(30);

        let mut decay_default = MemoryDecay::new(1.0);
        decay_default.decay_rate = 1.0;
        decay_default.last_accessed = Utc::now() - Duration::days(30);

        let mut decay_slow = MemoryDecay::new(1.0);
        decay_slow.decay_rate = 0.5;
        decay_slow.last_accessed = Utc::now() - Duration::days(30);

        let fast = decay_fast.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
        let default =
            decay_default.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
        let slow = decay_slow.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);

        assert!(
            fast < default,
            "rate=2.0 should decay faster: fast={} default={}",
            fast,
            default
        );
        assert!(
            slow > default,
            "rate=0.5 should decay slower: slow={} default={}",
            slow,
            default
        );
        // Exact: at 30 days with HL=30 and rate=1.0 → 0.5; rate=2.0 (HL=15) → 0.25; rate=0.5 (HL=60) → ~0.707
        assert!((default - 0.5).abs() < 0.01);
        assert!((fast - 0.25).abs() < 0.01);
        assert!((slow - 0.707).abs() < 0.01);
    }

    #[test]
    fn test_decay_formula_correctness() {
        // With base=1.0, 0 accesses, half_life=30:
        //   day 0  → 1.0 * 1.0  * 1.0 = 1.0
        //   day 30 → 1.0 * 0.5  * 1.0 = 0.5  (half-life property)
        //   day 60 → 1.0 * 0.25 * 1.0 = 0.25
        let mut decay = MemoryDecay::new(1.0);
        decay.access_count = 0;

        decay.last_accessed = Utc::now();
        let importance_day_0 =
            decay.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
        assert!(
            (importance_day_0 - 1.0).abs() < 0.001,
            "Day 0 should be ~1.0 (no decay yet), got {}",
            importance_day_0
        );

        decay.last_accessed = Utc::now() - Duration::days(30);
        let importance_day_30 =
            decay.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
        assert!(
            (importance_day_30 - 0.5).abs() < 0.01,
            "Day 30 at half_life=30 must be exactly half (0.5), got {}",
            importance_day_30
        );

        decay.last_accessed = Utc::now() - Duration::days(60);
        let importance_day_60 =
            decay.calculate_current_importance(1.0, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
        assert!(
            (importance_day_60 - 0.25).abs() < 0.01,
            "Day 60 at half_life=30 must be quarter (0.25), got {}",
            importance_day_60
        );

        assert!(importance_day_0 > importance_day_30);
        assert!(importance_day_30 > importance_day_60);
    }

    #[test]
    fn test_zero_access_young_memory_has_meaningful_importance() {
        // Regression test for silent no-op bug: prior formula `ln(access_count + 1)`
        // returned 0 for every memory in production because record_access was never
        // wired into retrieval paths. A fresh memory must score at base_importance,
        // not at the min_threshold floor.
        let decay = MemoryDecay::new(0.7);
        let min_threshold = 0.05;
        let importance =
            decay.calculate_current_importance(0.7, min_threshold, HALF_LIFE_DAYS, BOOST_FACTOR);

        assert!(
            importance > min_threshold,
            "Young 0-access memory must exceed floor, got {} (floor={})",
            importance,
            min_threshold
        );
        assert!(
            (importance - 0.7).abs() < 0.01,
            "Young 0-access memory should equal base_importance (0.7), got {}",
            importance
        );
    }

    #[test]
    fn test_importance_ranking_differentiates_base_importance() {
        // Pre-fix the broken `ln(access_count + 1) = 0` term zeroed every memory's
        // importance and clamped it to min_threshold, so every importance-weighted
        // ranking in store.rs was effectively comparing constants. This test asserts
        // the property all four importance-weighted code paths depend on: same-age,
        // same-access memories rank by base_importance strictly.
        let bases = [0.1_f32, 0.3, 0.5, 0.7, 0.9];
        let min_threshold = 0.05;

        let importances: Vec<f32> = bases
            .iter()
            .map(|&base| {
                let metadata = MemoryMetadata {
                    importance: base,
                    decay: MemoryDecay::new(base),
                    ..Default::default()
                };
                let memory = Memory::new(
                    MemoryType::Code,
                    "T".to_string(),
                    "C".to_string(),
                    Some(metadata),
                );
                memory.get_current_importance(true, min_threshold, HALF_LIFE_DAYS, BOOST_FACTOR)
            })
            .collect();

        for (i, &v) in importances.iter().enumerate() {
            assert!(
                v > min_threshold,
                "Memory {} (base={}) clamped to floor — ranking impossible",
                i,
                bases[i]
            );
        }
        for i in 1..importances.len() {
            assert!(
                importances[i] > importances[i - 1],
                "Rank broken: base[{}]={} > base[{}]={} but imp {} <= {}",
                i,
                bases[i],
                i - 1,
                bases[i - 1],
                importances[i],
                importances[i - 1]
            );
        }
    }

    #[test]
    fn test_multiple_accesses_compound_boost() {
        let mut decay = MemoryDecay::new(0.5);
        decay.last_accessed = Utc::now() - Duration::days(30);

        let mut importances = Vec::new();
        for count in [1, 5, 10, 20, 50] {
            decay.access_count = count;
            let importance =
                decay.calculate_current_importance(0.5, 0.0, HALF_LIFE_DAYS, BOOST_FACTOR);
            importances.push(importance);
        }

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
    fn test_access_boost_factor_scales_boost() {
        // Verify the config knob actually does something: a higher boost_factor
        // produces a higher importance for the same access_count.
        let mut decay = MemoryDecay::new(0.5);
        decay.last_accessed = Utc::now() - Duration::days(30);
        decay.access_count = 10;

        let with_zero_boost = decay.calculate_current_importance(0.5, 0.0, HALF_LIFE_DAYS, 0.0);
        let with_default_boost = decay.calculate_current_importance(0.5, 0.0, HALF_LIFE_DAYS, 1.2);
        let with_double_boost = decay.calculate_current_importance(0.5, 0.0, HALF_LIFE_DAYS, 2.4);

        // boost_factor=0 → access_boost term collapses to 1.0
        assert!(
            (with_zero_boost - 0.25).abs() < 0.01,
            "boost_factor=0 should give base * time_decay = 0.5 * 0.5 = 0.25, got {}",
            with_zero_boost
        );
        assert!(with_default_boost > with_zero_boost);
        assert!(with_double_boost > with_default_boost);
    }

    #[test]
    fn test_half_life_config_scales_decay() {
        // Verify the config knob actually does something: shorter half_life decays faster.
        let mut decay = MemoryDecay::new(1.0);
        decay.last_accessed = Utc::now() - Duration::days(60);
        decay.access_count = 0;

        let short = decay.calculate_current_importance(1.0, 0.0, 30, BOOST_FACTOR); // 60d / 30d HL = 2 half-lives → 0.25
        let medium = decay.calculate_current_importance(1.0, 0.0, 60, BOOST_FACTOR); // 60d / 60d HL = 1 half-life → 0.5
        let long = decay.calculate_current_importance(1.0, 0.0, 120, BOOST_FACTOR); // 60d / 120d HL = 0.5 half-life → ~0.707

        assert!(
            (short - 0.25).abs() < 0.01,
            "60-day-old at 30-day HL must equal 0.25, got {}",
            short
        );
        assert!(
            (medium - 0.5).abs() < 0.01,
            "60-day-old at 60-day HL must equal 0.5, got {}",
            medium
        );
        assert!(
            (long - 0.707).abs() < 0.01,
            "60-day-old at 120-day HL must be ~0.707, got {}",
            long
        );
    }

    #[test]
    fn test_decay_disabled_returns_base_importance() {
        let mut metadata = MemoryMetadata {
            importance: 0.75,
            decay: MemoryDecay::new(0.75),
            ..Default::default()
        };
        metadata.decay.last_accessed = Utc::now() - Duration::days(365); // Very old

        let memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            Some(metadata),
        );

        // With decay disabled, should always return base importance
        let importance = memory.get_current_importance(false, 0.05, HALF_LIFE_DAYS, BOOST_FACTOR);
        assert_eq!(importance, 0.75);
    }

    // Hybrid Search Query Tests

    #[test]
    fn test_hybrid_query_default_weights() {
        let query = HybridSearchQuery::default();

        assert_eq!(query.vector_weight, 0.8);
        assert_eq!(query.recency_weight, 0.1);
        assert_eq!(query.importance_weight, 0.1);

        // Weights should sum to 1.0
        let sum = query.vector_weight + query.recency_weight + query.importance_weight;
        assert!(
            (sum - 1.0).abs() < 0.001,
            "Weights should sum to 1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_weight_validation() {
        let valid_query = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            vector_weight: 0.8,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(valid_query.validate().is_ok());

        // Invalid: weight > 1.0
        let invalid_query = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            vector_weight: 1.5,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query.validate().is_err());

        // Invalid: weight < 0.0
        let invalid_query2 = HybridSearchQuery {
            vector_query: Some("test".to_string()),
            vector_weight: 0.5,
            recency_weight: -0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query2.validate().is_err());

        // Invalid: no vector_query
        let invalid_query3 = HybridSearchQuery {
            vector_query: None,
            vector_weight: 0.8,
            recency_weight: 0.1,
            importance_weight: 0.1,
            filters: Default::default(),
        };
        assert!(invalid_query3.validate().is_err());
    }
}
