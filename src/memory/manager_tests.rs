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
	pub(crate) async fn for_scope_test(db: &std::path::Path, scope: Option<String>, role: Option<String>) -> Self {
		let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
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

fn note(title: &str) -> MemorizeParams {
	MemorizeParams {
		memory_type: MemoryType::Insight,
		title: title.to_string(),
		content: title.to_string(),
		importance: None,
		tags: None,
		related_files: None,
		source: None,
	}
}

#[tokio::test]
async fn project_writes_keep_global_rows_global() {
	let root = tempfile::tempdir().unwrap();
	let db = root.path().join("memory");
	let mut global = MemoryManager::for_scope_test(&db, Some(String::new()), None).await;
	let mut alpha = MemoryManager::for_scope_test(&db, Some("alpha".to_string()), None).await;
	let beta = MemoryManager::for_scope_test(&db, Some("beta".to_string()), None).await;

	let shared = global.memorize(note("shared fact")).await.unwrap();
	let own = alpha.memorize(note("alpha fact")).await.unwrap();

	// An update from a project rewrites the row but must not move it out of global.
	assert!(alpha.add_tag(&shared.id, "seen".to_string()).await.unwrap());
	let updated = beta.store.get_memory(&shared.id).await.unwrap().unwrap();
	assert_eq!(updated.scope.as_deref(), Some(""));
	assert!(updated.metadata.tags.contains(&"seen".to_string()));

	// A bulk delete stays in the project; a by-id delete reaches global rows too.
	let query = MemoryQuery {
		query_text: Some("fact".to_string()),
		min_relevance: Some(0.0),
		..Default::default()
	};
	assert_eq!(alpha.forget_matching(query).await.unwrap(), 1);
	assert!(alpha.store.get_memory(&own.id).await.unwrap().is_none());
	assert!(beta.store.get_memory(&shared.id).await.unwrap().is_some());
	alpha.forget(&shared.id).await.unwrap();
	assert!(beta.store.get_memory(&shared.id).await.unwrap().is_none());
	assert!(alpha.forget(&shared.id).await.is_err());
}
