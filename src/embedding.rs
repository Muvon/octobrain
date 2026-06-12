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

/// Generate embeddings for a single text, with optional timeout from config.
pub async fn generate_embedding(
    text: &str,
    provider: &dyn EmbeddingProvider,
    timeout_secs: u64,
) -> anyhow::Result<Vec<f32>> {
    let fut = provider.generate_embedding(text);
    if timeout_secs == 0 {
        fut.await
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
            .await
            .map_err(|_| {
                anyhow::anyhow!("Embedding generation timed out after {}s", timeout_secs)
            })?
    }
}

/// Generate embeddings for multiple texts using batch API, with optional timeout from config.
pub async fn generate_embeddings_batch(
    texts: Vec<String>,
    provider: &dyn EmbeddingProvider,
    timeout_secs: u64,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let fut = provider.generate_embeddings_batch(texts, InputType::None);
    if timeout_secs == 0 {
        fut.await
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Batch embedding generation timed out after {}s",
                    timeout_secs
                )
            })?
    }
}
