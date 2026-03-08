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
    use super::super::store::MemoryStore;
    use super::super::types::{Memory, MemoryType};
    use chrono::{Duration, Utc};

    #[test]
    fn test_recency_score_new_memory() {
        let memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );
        let score = MemoryStore::calculate_recency_score(&memory, 30);
        assert!(
            score > 0.99,
            "New memory should have score ~1.0, got {}",
            score
        );
    }

    #[test]
    fn test_recency_score_old_memory() {
        let mut memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );
        memory.created_at = Utc::now() - Duration::days(90);
        let score = MemoryStore::calculate_recency_score(&memory, 30);
        // e^-3 ≈ 0.05
        assert!(
            score < 0.1,
            "Old memory should have low score, got {}",
            score
        );
        assert!(score > 0.0, "Score should be positive, got {}", score);
    }

    #[test]
    fn test_recency_score_decay_curve() {
        let mut memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );
        memory.created_at = Utc::now() - Duration::days(30);
        let score_30 = MemoryStore::calculate_recency_score(&memory, 30);
        // e^-1 ≈ 0.368
        assert!(
            (score_30 - 0.368).abs() < 0.01,
            "Expected ~0.368, got {}",
            score_30
        );

        memory.created_at = Utc::now() - Duration::days(60);
        let score_60 = MemoryStore::calculate_recency_score(&memory, 30);
        // e^-2 ≈ 0.135
        assert!(
            (score_60 - 0.135).abs() < 0.01,
            "Expected ~0.135, got {}",
            score_60
        );

        assert!(score_30 > score_60);
    }

    #[test]
    fn test_recency_score_future_timestamp() {
        let mut memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );
        memory.created_at = Utc::now() + Duration::days(10);
        let score = MemoryStore::calculate_recency_score(&memory, 30);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn test_recency_score_normalization() {
        let mut memory = Memory::new(
            MemoryType::Code,
            "Test".to_string(),
            "Content".to_string(),
            None,
        );
        for days in [0, 10, 30, 60, 90, 180, 365] {
            memory.created_at = Utc::now() - Duration::days(days);
            let score = MemoryStore::calculate_recency_score(&memory, 30);
            assert!(
                (0.0..=1.0).contains(&score),
                "Score for {} days should be in [0,1], got {}",
                days,
                score
            );
        }
    }
}
