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
        let pred = build_scalar_predicate_test(Some("proj123"), None, &query);
        assert_eq!(
            pred,
            "(scope = 'proj123' OR scope = '') AND state != 'archived'"
        );
        assert!(
            !pred.contains("role"),
            "No role filter expected when role is None"
        );
    }

    #[test]
    fn test_predicate_excludes_archived_always() {
        // Archived tombstones must never surface in relevance search, regardless
        // of scope/role/filters — the clause is unconditional.
        let query = MemoryQuery::default();
        assert!(build_scalar_predicate_test(Some("proj123"), None, &query)
            .contains("state != 'archived'"));
        assert!(build_scalar_predicate_test(None, None, &query).contains("state != 'archived'"));
    }

    #[test]
    fn test_predicate_with_role() {
        let query = MemoryQuery::default();
        let pred = build_scalar_predicate_test(Some("proj123"), Some("developer"), &query);
        assert!(
            pred.contains("role = 'developer'"),
            "Expected role filter in predicate, got: {}",
            pred
        );
        assert!(
            pred.starts_with("(scope = 'proj123' OR scope = '')"),
            "scope must be first condition"
        );
    }

    #[test]
    fn test_predicate_role_and_memory_type() {
        use super::super::types::MemoryType;
        let query = MemoryQuery {
            memory_types: Some(vec![MemoryType::Code]),
            ..Default::default()
        };
        let pred = build_scalar_predicate_test(Some("proj123"), Some("reviewer"), &query);
        assert!(pred.contains("scope = 'proj123'"));
        assert!(pred.contains("role = 'reviewer'"));
        assert!(pred.contains("memory_type IN ('code')"));
    }

    #[test]
    fn test_predicate_no_scope() {
        let query = MemoryQuery::default();
        let pred = build_scalar_predicate_test(None, None, &query);
        assert!(
            !pred.contains("scope"),
            "No scope filter expected when None, got: {}",
            pred
        );
    }

    #[test]
    fn test_predicate_role_none_with_filters() {
        use super::super::types::MemoryType;
        let query = MemoryQuery {
            memory_types: Some(vec![MemoryType::Architecture]),
            ..Default::default()
        };
        let pred = build_scalar_predicate_test(Some("myscope"), None, &query);
        assert!(!pred.contains("role"), "No role clause when role is None");
        assert!(pred.contains("memory_type IN ('architecture')"));
    }
}
