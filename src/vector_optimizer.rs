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

use lancedb::{query::VectorQuery, DistanceType, Table};

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
    /// Calculate optimal index parameters based on dataset size
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

    /// Check if index should be optimized due to dataset growth
    pub fn should_optimize_for_growth(
        _row_count: usize,
        _vector_dim: usize,
        _has_index: bool,
    ) -> bool {
        // For simplicity, don't auto-optimize in octobrain
        // Users can manually recreate index if needed
        false
    }

    /// Optimize query parameters
    pub async fn optimize_query(
        query: VectorQuery,
        _table: &Table,
        _table_name: &str,
    ) -> Result<VectorQuery, anyhow::Error> {
        // Return query as-is for now
        // Could add query optimization logic here if needed
        Ok(query)
    }
}
