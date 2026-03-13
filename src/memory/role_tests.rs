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
    use super::super::store::build_scalar_predicate_test;
    use super::super::types::MemoryQuery;

    // build_scalar_predicate is private, so store.rs exposes a test-only re-export.

    #[test]
    fn test_predicate_no_role() {
        let query = MemoryQuery::default();
        let pred = build_scalar_predicate_test("proj123", None, &query);
        assert_eq!(pred, "project_key = 'proj123'");
        assert!(
            !pred.contains("role"),
            "No role filter expected when role is None"
        );
    }

    #[test]
    fn test_predicate_with_role() {
        let query = MemoryQuery::default();
        let pred = build_scalar_predicate_test("proj123", Some("developer"), &query);
        assert!(
            pred.contains("role = 'developer'"),
            "Expected role filter in predicate, got: {}",
            pred
        );
        assert!(
            pred.starts_with("project_key = 'proj123'"),
            "project_key must be first condition"
        );
    }

    #[test]
    fn test_predicate_role_and_memory_type() {
        use super::super::types::MemoryType;
        let query = MemoryQuery {
            memory_types: Some(vec![MemoryType::Code]),
            ..Default::default()
        };
        let pred = build_scalar_predicate_test("proj123", Some("reviewer"), &query);
        assert!(pred.contains("project_key = 'proj123'"));
        assert!(pred.contains("role = 'reviewer'"));
        assert!(pred.contains("memory_type IN ('code')"));
    }

    #[test]
    fn test_predicate_role_none_with_filters() {
        use super::super::types::MemoryType;
        let query = MemoryQuery {
            memory_types: Some(vec![MemoryType::Architecture]),
            ..Default::default()
        };
        let pred = build_scalar_predicate_test("myproject", None, &query);
        assert!(!pred.contains("role"), "No role clause when role is None");
        assert!(pred.contains("memory_type IN ('architecture')"));
    }
}
