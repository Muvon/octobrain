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

use super::*;
use octolib::embedding::{EmbeddingProvider, EmbeddingUsage, InputType};

struct FakeEmbedding;

#[async_trait::async_trait]
impl EmbeddingProvider for FakeEmbedding {
    async fn generate_embedding(&self, _text: &str) -> Result<(Vec<f32>, EmbeddingUsage)> {
        Ok((
            vec![1.0, 0.0],
            EmbeddingUsage {
                input_tokens: 1,
                cost: None,
            },
        ))
    }

    async fn generate_embeddings_batch(
        &self,
        texts: Vec<String>,
        _input_type: InputType,
    ) -> Result<(Vec<Vec<f32>>, EmbeddingUsage)> {
        Ok((
            vec![vec![1.0, 0.0]; texts.len()],
            EmbeddingUsage {
                input_tokens: texts.len() as u64,
                cost: None,
            },
        ))
    }

    fn get_dimension(&self) -> usize {
        2
    }
}

impl MemoryManager {
    pub(crate) async fn for_scope_test(
        db: &std::path::Path,
        scope: Option<String>,
        role: Option<String>,
    ) -> Self {
        let mut config: Config =
            toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
        config.memory.auto_linking_enabled = false;
        config.search.hyde.enabled = false;
        config.search.reranker.enabled = false;
        config.search.hybrid.enabled = false;
        let store = MemoryStore::new(
            db.to_str().unwrap(),
            scope,
            role,
            Arc::new(FakeEmbedding),
            config.memory.clone(),
            config.clone(),
            None,
        )
        .await
        .unwrap();
        Self {
            store: Arc::new(store),
            config: config.memory,
            stale_check_marker: db.join("stale"),
            sleep_consolidation_marker: db.join("sleep"),
            pending_auto_links: Arc::new(AsyncMutex::new(Vec::new())),
            memorize_counter: Arc::new(AtomicUsize::new(0)),
            pending_maintenance: Arc::new(AsyncMutex::new(None)),
        }
    }
}
