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

use crate::config::Config;
use anyhow::Result;

// Re-export embedding functionality from octolib
pub use octolib::embedding::{
    create_embedding_provider_from_parts, parse_provider_model, provider::EmbeddingProvider,
};

/// Generate embeddings for text content
pub async fn generate_embeddings(contents: &str, config: &Config) -> Result<Vec<f32>> {
    // Get model string from config
    let model_string = &config.embedding.model;

    // Parse provider and model from string
    let (provider, model) = if let Some((p, m)) = model_string.split_once(':') {
        (p, m)
    } else {
        return Err(anyhow::anyhow!("Invalid model format: {}", model_string));
    };

    octolib::embedding::generate_embeddings(contents, provider, model).await
}

/// Truncate output to a maximum number of tokens (approximate)
/// Uses simple character-based estimation: ~4 chars per token
pub fn truncate_output(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return text.to_string();
    }

    let max_chars = max_tokens * 4; // Approximate: 4 chars per token
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}...[truncated]", &text[..max_chars])
    }
}
