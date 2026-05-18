// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

#[cfg(test)]
mod tests {
    use super::super::types::{MemoryState, MemoryType, RelationshipType};

    #[test]
    fn test_memory_type_goal_round_trip() {
        let t = MemoryType::Goal;
        assert_eq!(t.to_string(), "goal");
        assert_eq!(MemoryType::from("goal".to_string()), MemoryType::Goal);
        // Aliases must also resolve to Goal so MCP/CLI callers can use natural language.
        assert_eq!(MemoryType::from("intent".to_string()), MemoryType::Goal);
        assert_eq!(MemoryType::from("task".to_string()), MemoryType::Goal);
        assert_eq!(MemoryType::from("objective".to_string()), MemoryType::Goal);
    }

    #[test]
    fn test_memory_state_default_is_working() {
        // Default state must be Working so existing memories and new inserts both
        // participate in retrieval normally — Consolidated/Archived are opt-in
        // transitions driven by consolidate_goal / cleanup paths.
        assert_eq!(MemoryState::default(), MemoryState::Working);
    }

    #[test]
    fn test_memory_state_round_trip_via_string() {
        for state in [
            MemoryState::Working,
            MemoryState::Consolidated,
            MemoryState::Archived,
        ] {
            let s = state.to_string();
            let back = MemoryState::from(s.clone());
            assert_eq!(
                back, state,
                "MemoryState '{}' must round-trip through string",
                s
            );
        }
    }

    #[test]
    fn test_memory_state_unknown_string_defaults_to_working() {
        // Unknown / legacy values become Working so legacy rows never silently
        // disappear from retrieval due to a state column drift.
        assert_eq!(
            MemoryState::from("totally_unknown".to_string()),
            MemoryState::Working
        );
        assert_eq!(MemoryState::from("".to_string()), MemoryState::Working);
    }

    #[test]
    fn test_relationship_achieves_and_closes_round_trip() {
        // Display → From<&str> must round-trip for the goal-consolidation types
        // so stored relationships read back as the right variant.
        for rel in [
            RelationshipType::Achieves,
            RelationshipType::Closes,
            RelationshipType::RelatedTo,
            RelationshipType::AutoLinked,
        ] {
            let s = rel.to_string();
            let back = RelationshipType::from(s.as_str());
            assert_eq!(
                format!("{}", back),
                s,
                "RelationshipType '{}' must round-trip",
                s
            );
        }
    }

    #[test]
    fn test_relationship_legacy_camelcase_still_parses() {
        // store_relationship used to write Display (snake_case) but the original
        // batch_to_relationships matched CamelCase — so legacy rows may exist in
        // either form. From<&str> must accept both.
        assert!(matches!(
            RelationshipType::from("Achieves"),
            RelationshipType::Achieves
        ));
        assert!(matches!(
            RelationshipType::from("Closes"),
            RelationshipType::Closes
        ));
        assert!(matches!(
            RelationshipType::from("RelatedTo"),
            RelationshipType::RelatedTo
        ));
        assert!(matches!(
            RelationshipType::from("AutoLinked"),
            RelationshipType::AutoLinked
        ));
    }

    #[test]
    fn test_relationship_unknown_becomes_custom() {
        match RelationshipType::from("some_user_defined_rel") {
            RelationshipType::Custom(s) => {
                assert_eq!(s, "some_user_defined_rel");
            }
            other => panic!("Expected Custom, got {:?}", other),
        }
    }
}
