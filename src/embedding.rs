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

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use octolib::embedding::types::EmbeddingProviderType;

mod shared;

// Re-export embedding functionality from octolib
pub use octolib::embedding::{
    parse_provider_model, provider::create_embedding_provider_from_parts,
    provider::EmbeddingProvider, types::InputType,
};

/// Local model providers cached per model string: their ONNX weights stay
/// resident (hundreds of MB), so all managers in the process share one instance.
static LOCAL_PROVIDER_CACHE: LazyLock<Mutex<HashMap<String, Arc<dyn EmbeddingProvider>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Create embedding provider from config.
///
/// In-process model providers (fastembed, huggingface) are cached for the
/// process lifetime, keyed by the configured model string — the memory and
/// knowledge managers then share one loaded model instead of one each.
/// API-backed providers are lightweight and constructed fresh every time.
pub async fn create_embedding_provider(
    config: &crate::config::Config,
) -> anyhow::Result<Arc<dyn EmbeddingProvider>> {
    let (provider, model) = parse_provider_model(&config.embedding.model)?;

    let is_local = matches!(
        &provider,
        EmbeddingProviderType::FastEmbed | EmbeddingProviderType::HuggingFace
    );
    if !is_local {
        let boxed = create_embedding_provider_from_parts(&provider, &model).await?;
        return Ok(Arc::from(boxed));
    }

    let key = config.embedding.model.clone();
    if let Some(cached) = LOCAL_PROVIDER_CACHE.lock().unwrap().get(&key) {
        return Ok(cached.clone());
    }

    // Local models: elect one process on this machine to load the weights
    // and serve inference; the rest attach as clients over loopback (see
    // `shared`). Only the elected process constructs the real provider.
    let built: Arc<dyn EmbeddingProvider> = match shared::join(&key, provider.clone(), &model).await
    {
        Ok(shared) => Arc::new(shared),
        Err(e) => {
            tracing::warn!("shared embedding service unavailable ({e:#}); loading a private model");
            Arc::from(create_embedding_provider_from_parts(&provider, &model).await?)
        }
    };
    // Keep the first instance if two constructions raced; the loser is dropped.
    let mut cache = LOCAL_PROVIDER_CACHE.lock().unwrap();
    Ok(cache.entry(key).or_insert(built).clone())
}

/// Generate embeddings for a single text, with optional timeout from config.
pub async fn generate_embedding(
    text: &str,
    provider: &dyn EmbeddingProvider,
    timeout_secs: u64,
) -> anyhow::Result<Vec<f32>> {
    let fut = provider.generate_embedding(text);
    let (embedding, _usage) = if timeout_secs == 0 {
        fut.await?
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
            .await
            .map_err(|_| {
                anyhow::anyhow!("Embedding generation timed out after {}s", timeout_secs)
            })??
    };
    Ok(embedding)
}

/// Generate embeddings for multiple texts using batch API, with optional timeout from config.
pub async fn generate_embeddings_batch(
    texts: Vec<String>,
    provider: &dyn EmbeddingProvider,
    timeout_secs: u64,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let fut = provider.generate_embeddings_batch(texts, InputType::None);
    let (embeddings, _usage) = if timeout_secs == 0 {
        fut.await?
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Batch embedding generation timed out after {}s",
                    timeout_secs
                )
            })??
    };
    Ok(embeddings)
}
