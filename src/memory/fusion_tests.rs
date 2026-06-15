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
    use super::super::manager::rrf_fuse;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_rrf_rank_zero_value() {
        // A single list, single id at rank 0 → 1/(60+0).
        let fused = rrf_fuse(&[ids(&["x"])]);
        assert!((fused["x"] - 1.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn test_rrf_rewards_consensus_over_one_off_top() {
        // "b" is rank-1 (2nd) in BOTH queries; "a" is rank-0 (1st) in only one and
        // absent from the other. RRF should rank the robust-across-queries "b" above
        // the one-off "a" — exactly what the old keep-max + flat-boost got wrong.
        let lists = vec![ids(&["a", "b"]), ids(&["c", "b"])];
        let fused = rrf_fuse(&lists);
        assert!(
            fused["b"] > fused["a"],
            "consensus #2 ({}) should beat one-off #1 ({})",
            fused["b"],
            fused["a"]
        );
        assert!(fused["b"] > fused["c"]);
    }

    #[test]
    fn test_rrf_accumulates_across_lists() {
        // Same id at rank 0 in two lists = 2 * 1/60.
        let fused = rrf_fuse(&[ids(&["z"]), ids(&["z"])]);
        assert!((fused["z"] - 2.0 / 60.0).abs() < 1e-6);
    }

    #[test]
    fn test_rrf_empty_input() {
        assert!(rrf_fuse(&[]).is_empty());
    }
}
