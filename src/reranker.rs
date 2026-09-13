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

//! Reranker provider seam for octobrain.
//!
//! Local (model-resident) rerankers are cached per model string: their ONNX
//! weights are large and slow to load, so memory and knowledge searches share
//! one loaded model instead of constructing — and reloading — one per search.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use octolib::reranker::{
    create_rerank_provider_from_parts, parse_provider_model, RerankProvider, RerankProviderType,
};

static LOCAL_RERANKER_CACHE: LazyLock<Mutex<HashMap<String, Arc<dyn RerankProvider>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Create a rerank provider from a fully qualified model string
/// (`provider:model`, e.g. `fastembed:jina-reranker-v1-turbo-en`).
///
/// In-process model providers (fastembed, huggingface) are cached for the
/// process lifetime, keyed by the model string; API-backed providers are
/// lightweight and constructed fresh every time.
pub async fn create_rerank_provider(model_string: &str) -> anyhow::Result<Arc<dyn RerankProvider>> {
    let (provider, model) = parse_provider_model(model_string)?;

    let is_local = matches!(
        &provider,
        RerankProviderType::FastEmbed | RerankProviderType::HuggingFace
    );
    if !is_local {
        let boxed = create_rerank_provider_from_parts(&provider, &model).await?;
        return Ok(Arc::from(boxed));
    }

    let key = model_string.to_string();
    if let Some(cached) = LOCAL_RERANKER_CACHE.lock().unwrap().get(&key) {
        return Ok(cached.clone());
    }
    let built = Arc::from(create_rerank_provider_from_parts(&provider, &model).await?);
    // Keep the first instance if two constructions raced; the loser is dropped.
    let mut cache = LOCAL_RERANKER_CACHE.lock().unwrap();
    Ok(cache.entry(key).or_insert(built).clone())
}
