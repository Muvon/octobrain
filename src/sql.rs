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

//! Shared SQL predicate helpers for LanceDB query construction.
//!
//! LanceDB executes predicates through DataFusion, whose SQL dialect escapes an
//! embedded single quote inside a string literal by doubling it (`'` -> `''`).
//! Both `MemoryStore` and `KnowledgeStore` interpolate user-controlled values
//! (memory IDs, project keys, roles, sources, session IDs) into predicates, so
//! they share this one escaping implementation to avoid drifting variants.

/// Escape a string for safe inclusion inside a LanceDB SQL single-quoted literal.
///
/// DataFusion escapes an embedded `'` by doubling it. Always wrap the result in
/// single quotes at the call site: `format!("col = '{}'", escape_sql_literal(v))`.
pub(crate) fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_single_quotes() {
        assert_eq!(escape_sql_literal("o'brien"), "o''brien");
    }

    #[test]
    fn leaves_clean_strings_untouched() {
        assert_eq!(escape_sql_literal("project-abc"), "project-abc");
    }

    #[test]
    fn neutralizes_injection_attempt() {
        assert_eq!(escape_sql_literal("x' OR '1'='1"), "x'' OR ''1''=''1");
    }
}
