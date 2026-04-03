// Copyright 2026 Muvon Un Limited
//
use lancedb::DistanceType;

/// Parameters for vector index optimization
pub struct IndexParams {
    pub should_create_index: bool,
    pub num_partitions: u32,
    pub num_sub_vectors: u32,
    pub num_bits: usize,
    pub distance_type: DistanceType,
}

/// Vector index optimizer for LanceDB
pub struct VectorOptimizer;

impl VectorOptimizer {
    /// Calculate optimal index parameters based on dataset size.
    /// Uses Dot distance for normalized embeddings (Voyage, OpenAI) —
    /// mathematically equivalent to Cosine but skips normalization at query time.
    pub fn calculate_index_params(row_count: usize, vector_dim: usize) -> IndexParams {
        // Don't create index for small datasets (< 1000 rows)
        if row_count < 1000 {
            return IndexParams {
                should_create_index: false,
                num_partitions: 0,
                num_sub_vectors: 0,
                num_bits: 0,
                distance_type: DistanceType::Cosine,
            };
        }

        // Calculate optimal partitions (sqrt of row count, min 2, max 256)
        let num_partitions = ((row_count as f64).sqrt() as u32).clamp(2, 256);

        // Calculate sub-vectors (vector_dim / 8, min 1, max 96)
        let num_sub_vectors = ((vector_dim / 8) as u32).clamp(1, 96);

        IndexParams {
            should_create_index: true,
            num_partitions,
            num_sub_vectors,
            num_bits: 8, // Standard 8-bit quantization
            distance_type: DistanceType::Cosine,
        }
    }

    /// Check if index should be optimized due to dataset growth.
    /// Check if index should be optimized due to dataset growth.
    /// Returns true when row_count crosses meaningful thresholds:
    /// - Every 500 rows when has_index is true (index may be stale)
    /// - Every 1000 rows when has_index is false (should have index)
    pub fn should_optimize_for_growth(
        row_count: usize,
        _vector_dim: usize,
        has_index: bool,
    ) -> bool {
        if has_index {
            // Index exists — recommend re-optimization every 500 new rows
            // to keep partition counts aligned with data size
            row_count.is_multiple_of(500) && row_count >= 1000
        } else {
            // No index yet — recommend creating one once we hit threshold
            row_count >= 1000 && row_count.is_multiple_of(1000)
        }
    }
}
