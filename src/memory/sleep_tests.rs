// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

#[cfg(test)]
mod tests {
    use super::super::manager::build_clusters;

    fn ids(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_no_candidates_yields_no_clusters() {
        let clusters = build_clusters(&[], 3);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cluster_below_min_size_is_discarded() {
        // A with one neighbor B (cluster size 2) — below min_size=3 → discarded
        let input = vec![("a".to_string(), ids(&["b"]))];
        let clusters = build_clusters(&input, 3);
        assert!(
            clusters.is_empty(),
            "Cluster of size 2 must be discarded when min_size=3"
        );
    }

    #[test]
    fn test_cluster_at_min_size_is_kept() {
        let input = vec![("a".to_string(), ids(&["b", "c"]))];
        let clusters = build_clusters(&input, 3);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 3);
        assert!(clusters[0].contains(&"a".to_string()));
        assert!(clusters[0].contains(&"b".to_string()));
        assert!(clusters[0].contains(&"c".to_string()));
    }

    #[test]
    fn test_member_claimed_by_first_cluster_does_not_join_second() {
        // a's cluster claims {a, b, c}. d's neighbor list also has b, but b is
        // already claimed → d's cluster only gets {d, e}. min_size=3 → discarded.
        let input = vec![
            ("a".to_string(), ids(&["b", "c"])),
            ("d".to_string(), ids(&["b", "e"])),
        ];
        let clusters = build_clusters(&input, 3);
        assert_eq!(clusters.len(), 1, "Only the first cluster should survive");
        assert!(clusters[0].contains(&"a".to_string()));
        assert!(!clusters[0].contains(&"d".to_string()));
    }

    #[test]
    fn test_two_disjoint_clusters_both_kept() {
        let input = vec![
            ("a".to_string(), ids(&["b", "c"])),
            ("d".to_string(), ids(&["e", "f"])),
        ];
        let clusters = build_clusters(&input, 3);
        assert_eq!(clusters.len(), 2);
        let total: usize = clusters.iter().map(|c| c.len()).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn test_duplicate_neighbor_ids_dedupe_within_cluster() {
        // Repeated neighbor IDs should be deduplicated within a cluster
        let input = vec![("a".to_string(), ids(&["b", "b", "c", "c"]))];
        let clusters = build_clusters(&input, 3);
        assert_eq!(clusters.len(), 1);
        assert_eq!(
            clusters[0].len(),
            3,
            "Dedup should produce exactly {{a, b, c}}, got {:?}",
            clusters[0]
        );
    }

    #[test]
    fn test_self_reference_in_neighbors_is_ignored() {
        // A pathological case where the candidate's own id appears in neighbors
        let input = vec![("a".to_string(), ids(&["a", "b", "c"]))];
        let clusters = build_clusters(&input, 3);
        assert_eq!(clusters.len(), 1);
        let cluster = &clusters[0];
        assert_eq!(cluster.len(), 3);
        let a_count = cluster.iter().filter(|x| x.as_str() == "a").count();
        assert_eq!(a_count, 1, "Self should appear exactly once");
    }

    #[test]
    fn test_clustering_is_disjoint() {
        // Verify the disjointness invariant: no id appears in more than one cluster
        let input = vec![
            ("a".to_string(), ids(&["b", "c", "d"])),
            ("e".to_string(), ids(&["f", "g", "h"])),
            ("i".to_string(), ids(&["j", "k"])),
        ];
        let clusters = build_clusters(&input, 3);

        let mut seen = std::collections::HashSet::new();
        for cluster in &clusters {
            for member in cluster {
                assert!(
                    seen.insert(member.clone()),
                    "Member '{}' appears in multiple clusters",
                    member
                );
            }
        }
    }
}
