// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

#[cfg(test)]
mod tests {
    use super::super::store::rocchio_blend;
    use crate::config::HydeConfig;

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
        dot / (na * nb)
    }

    #[test]
    fn test_rocchio_alpha_one_returns_normalized_query() {
        // alpha = 1.0 → centroid term zeroed out, result = normalized query
        let q = vec![3.0, 4.0, 0.0]; // norm = 5
        let centroid = vec![10.0, 20.0, 30.0];
        let out = rocchio_blend(&q, &centroid, 1.0);
        assert!((out[0] - 0.6).abs() < 0.001);
        assert!((out[1] - 0.8).abs() < 0.001);
        assert!((out[2] - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_rocchio_alpha_zero_returns_normalized_centroid() {
        let q = vec![1.0, 0.0, 0.0];
        let centroid = vec![0.0, 3.0, 4.0]; // norm = 5
        let out = rocchio_blend(&q, &centroid, 0.0);
        assert!((out[0] - 0.0).abs() < 0.001);
        assert!((out[1] - 0.6).abs() < 0.001);
        assert!((out[2] - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_rocchio_alpha_half_blends() {
        let q = vec![1.0, 0.0];
        let centroid = vec![0.0, 1.0];
        let out = rocchio_blend(&q, &centroid, 0.5);
        // blend = (0.5, 0.5); norm = sqrt(0.5) → each component = 0.5/sqrt(0.5) ≈ 0.707
        let expected = (0.5_f32).sqrt();
        assert!((out[0] - expected).abs() < 0.001);
        assert!((out[1] - expected).abs() < 0.001);
    }

    #[test]
    fn test_rocchio_clamps_alpha() {
        let q = vec![1.0, 0.0];
        let centroid = vec![0.0, 1.0];
        // Negative alpha is clamped to 0 → output = normalized centroid
        let out_neg = rocchio_blend(&q, &centroid, -5.0);
        assert!((out_neg[1] - 1.0).abs() < 0.001);
        // alpha > 1 clamped to 1 → output = normalized query
        let out_over = rocchio_blend(&q, &centroid, 99.0);
        assert!((out_over[0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_rocchio_output_is_unit_length() {
        let q = vec![0.6, 0.8, 0.0]; // norm = 1.0
        let centroid = vec![0.0, 0.6, 0.8];
        let out = rocchio_blend(&q, &centroid, 0.7);
        let norm: f32 = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.001,
            "Output should be unit vector, got norm={}",
            norm
        );
    }

    /// The core property PRF is supposed to deliver: when the original query is noisy,
    /// blending in the centroid of relevant neighbors moves the query closer to the
    /// truly-relevant docs in embedding space — improving similarity to a target.
    #[test]
    fn test_rocchio_pulls_noisy_query_toward_relevant_cluster() {
        // True intent direction: [1, 0, 0]
        // Noisy query: 70% intent, 30% noise on an orthogonal axis
        let noisy_query = vec![0.7_f32, 0.0, 0.3];
        // Top-K relevant docs cluster around the true intent
        let docs = [
            vec![0.95_f32, 0.05, 0.0],
            vec![0.9, 0.0, 0.05],
            vec![0.92, 0.05, 0.02],
        ];
        let target = vec![1.0_f32, 0.0, 0.0]; // the true relevant doc we want to find

        // Centroid of the top-K
        let mut centroid = vec![0.0_f32; 3];
        for d in &docs {
            for (i, v) in d.iter().enumerate() {
                centroid[i] += v;
            }
        }
        for v in &mut centroid {
            *v /= docs.len() as f32;
        }

        let original_sim = cos(&noisy_query, &target);
        let expanded = rocchio_blend(&noisy_query, &centroid, 0.5);
        let expanded_sim = cos(&expanded, &target);

        assert!(
            expanded_sim > original_sim,
            "PRF should pull noisy query closer to target: orig_sim={:.4}, expanded_sim={:.4}",
            original_sim,
            expanded_sim
        );
    }

    #[test]
    fn test_hyde_config_default_is_enabled() {
        // Default is ON: autonomous improvement, no LLM dependency.
        // Costs one extra LanceDB vector query per search for typically
        // +10-30% recall on long-tail queries.
        let cfg = HydeConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.top_k, 3);
        assert!((cfg.alpha - 0.5).abs() < 0.001);
    }
}
