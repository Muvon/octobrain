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

    #[test]
    fn test_keyword_tokenization() {
        let text = "Hello, World! This is a test-case with_underscores.";
        let tokens = MemoryStore::tokenize(text);

        assert_eq!(
            tokens,
            vec![
                "hello",
                "world",
                "this",
                "is",
                "a",
                "test",
                "case",
                "with_underscores"
            ]
        );
    }

    #[test]
    fn test_keyword_tokenization_empty() {
        let tokens = MemoryStore::tokenize("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_keyword_tokenization_punctuation() {
        let text = "!!!???...";
        let tokens = MemoryStore::tokenize(text);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tf_calculation() {
        let text = "rust programming rust language rust";
        let tf = MemoryStore::calculate_tf("rust", text);

        // "rust" appears 3 times out of 5 words
        assert!((tf - 0.6).abs() < 0.01, "Expected ~0.6, got {}", tf);
    }

    #[test]
    fn test_tf_calculation_no_match() {
        let text = "hello world";
        let tf = MemoryStore::calculate_tf("rust", text);
        assert_eq!(tf, 0.0);
    }

    #[test]
    fn test_tf_calculation_case_insensitive() {
        let text = "Rust RUST rust RuSt";
        let tf = MemoryStore::calculate_tf("rust", text);
        assert_eq!(tf, 1.0); // All 4 words match
    }

    #[test]
    fn test_keyword_scoring_title_boost() {
        let keywords = vec!["rust".to_string()];

        // Title has higher weight (3.0) than content (1.0)
        let title_score = MemoryStore::score_field(&keywords, "rust programming", 3.0);
        let content_score = MemoryStore::score_field(&keywords, "rust programming", 1.0);

        assert!(title_score > content_score);
        assert!((title_score / content_score - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_keyword_scoring_tags_boost() {
        let keywords = vec!["rust".to_string()];

        // Tags have weight 2.0
        let tags_score = MemoryStore::score_field(&keywords, "rust programming", 2.0);
        let content_score = MemoryStore::score_field(&keywords, "rust programming", 1.0);

        assert!(tags_score > content_score);
        assert!((tags_score / content_score - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_keyword_no_matches() {
        let keywords = vec!["nonexistent".to_string()];
        let score = MemoryStore::score_field(&keywords, "hello world", 1.0);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_keyword_empty_keywords() {
        let keywords: Vec<String> = vec![];
        let score = MemoryStore::score_field(&keywords, "hello world", 1.0);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_keyword_empty_text() {
        let keywords = vec!["rust".to_string()];
        let score = MemoryStore::score_field(&keywords, "", 1.0);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_keyword_multiple_keywords() {
        let keywords = vec!["rust".to_string(), "programming".to_string()];
        let text = "rust programming language";
        let score = MemoryStore::score_field(&keywords, text, 1.0);

        // Both keywords match, score should be sum of TFs
        // "rust": 1/3, "programming": 1/3, total: 2/3
        assert!(
            (score - 0.666).abs() < 0.01,
            "Expected ~0.666, got {}",
            score
        );
    }

    // Recency Scoring Tests

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

        // New memory should have score close to 1.0
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

        // Set created_at to 90 days ago
        memory.created_at = Utc::now() - Duration::days(90);

        let score = MemoryStore::calculate_recency_score(&memory, 30);

        // After 90 days with decay_days=30, score should be e^-3 ≈ 0.05
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

        // Test at decay half-life (30 days)
        memory.created_at = Utc::now() - Duration::days(30);
        let score_30 = MemoryStore::calculate_recency_score(&memory, 30);

        // At 30 days with decay=30, score should be e^-1 ≈ 0.368
        assert!(
            (score_30 - 0.368).abs() < 0.01,
            "Expected ~0.368, got {}",
            score_30
        );

        // Test at 60 days
        memory.created_at = Utc::now() - Duration::days(60);
        let score_60 = MemoryStore::calculate_recency_score(&memory, 30);

        // At 60 days, score should be e^-2 ≈ 0.135
        assert!(
            (score_60 - 0.135).abs() < 0.01,
            "Expected ~0.135, got {}",
            score_60
        );

        // Verify decay: older memories have lower scores
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

        // Set created_at to future (shouldn't happen, but handle gracefully)
        memory.created_at = Utc::now() + Duration::days(10);

        let score = MemoryStore::calculate_recency_score(&memory, 30);

        // Future timestamp should return 1.0
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

        // Test various ages
        for days in [0, 10, 30, 60, 90, 180, 365] {
            memory.created_at = Utc::now() - Duration::days(days);
            let score = MemoryStore::calculate_recency_score(&memory, 30);

            // Score should always be in [0.0, 1.0]
            assert!(
                (0.0..=1.0).contains(&score),
                "Score for {} days should be in [0,1], got {}",
                days,
                score
            );
        }
    }
}
