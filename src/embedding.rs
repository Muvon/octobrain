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

// Re-export embedding functionality from octolib
pub use octolib::embedding::{
    parse_provider_model, provider::create_embedding_provider_from_parts,
    provider::EmbeddingProvider, types::InputType,
};

/// Create embedding provider from config
pub async fn create_embedding_provider(
    config: &crate::config::Config,
) -> anyhow::Result<Box<dyn EmbeddingProvider>> {
    let (provider, model) = parse_provider_model(&config.embedding.model)?;
    create_embedding_provider_from_parts(&provider, &model).await
}

/// Generate embeddings for a single text
pub async fn generate_embedding(
    text: &str,
    provider: &dyn EmbeddingProvider,
) -> anyhow::Result<Vec<f32>> {
    provider.generate_embedding(text).await
}

/// Generate embeddings for multiple texts using batch API
pub async fn generate_embeddings_batch(
    texts: Vec<String>,
    provider: &dyn EmbeddingProvider,
) -> anyhow::Result<Vec<Vec<f32>>> {
    provider
        .generate_embeddings_batch(texts, InputType::None)
        .await
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
