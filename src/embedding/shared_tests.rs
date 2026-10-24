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

use anyhow::Result;
use octolib::embedding::{EmbeddingProvider, EmbeddingUsage};

use super::super::InputType;
use super::{endpoint_path, join_in, read_endpoint, BuildInner, Endpoint};

struct FakeProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for FakeProvider {
	async fn generate_embedding(&self, text: &str) -> Result<(Vec<f32>, EmbeddingUsage)> {
		Ok((
			vec![text.len() as f32, 1.0],
			EmbeddingUsage {
				input_tokens: 3,
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
			texts.iter().map(|t| vec![t.len() as f32, 2.0]).collect(),
			EmbeddingUsage {
				input_tokens: 7,
				cost: None,
			},
		))
	}

	fn get_dimension(&self) -> usize {
		2
	}

	async fn model_revision(&self) -> Result<Option<String>> {
		Ok(Some("fakerev".to_string()))
	}
}

fn fake_build() -> BuildInner {
	std::sync::Arc::new(|| Box::pin(async { Ok(Box::new(FakeProvider) as Box<dyn EmbeddingProvider>) }))
}

#[tokio::test]
async fn second_participant_becomes_a_client_of_the_owner() {
	let dir = tempfile::tempdir().unwrap();
	let key = format!("test-shared-{}", std::process::id());
	let first = join_in(dir.path(), &key, fake_build()).await.expect("owner elected");
	let second = join_in(dir.path(), &key, fake_build()).await.expect("client attaches");
	assert!(!second.owns_model().await);

	// Client round-trips through the owner's FakeProvider.
	let (vector, usage) = second.generate_embedding("abcd").await.expect("remote embed");
	assert_eq!(vector, vec![4.0, 1.0]);
	assert_eq!(usage.input_tokens, 3);

	let (vectors, _) = second
		.generate_embeddings_batch(vec!["ab".to_string(), "abc".to_string()], InputType::Query)
		.await
		.expect("remote batch");
	assert_eq!(vectors, vec![vec![2.0, 2.0], vec![3.0, 2.0]]);

	// Dimension and revision come from the handshake, not local weights.
	assert_eq!(second.get_dimension(), 2);
	assert_eq!(second.model_revision().await.unwrap(), Some("fakerev".to_string()));

	// Owner answers locally, identically.
	let (local, _) = first.generate_embedding("abcd").await.expect("owner embed");
	assert_eq!(local, vector);
}

#[tokio::test]
async fn stale_endpoint_is_replaced() {
	let dir = tempfile::tempdir().unwrap();
	let key = format!("test-stale-{}", std::process::id());
	let path = endpoint_path(dir.path(), &key).unwrap();
	let dead = Endpoint {
		port: 1,
		token: "stale".to_string(),
		model: key.clone(),
		pid: 0,
	};
	std::fs::write(&path, serde_json::to_string(&dead).unwrap()).unwrap();

	let elected = join_in(dir.path(), &key, fake_build())
		.await
		.expect("recover from stale endpoint");
	assert!(elected.owns_model().await);
	let current = read_endpoint(&path, &key).expect("endpoint rewritten");
	assert_ne!(current.token, "stale");
	assert_eq!(current.pid, std::process::id());
}

// prod 2026-10-24: the owner is just another octobrain process. When it exited,
// every client kept dialling its dead port (`Connection refused`) for the rest of
// its life. Now the next request re-elects and is answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_outlives_its_owner() {
	let dir = tempfile::tempdir().unwrap();
	let key = format!("test-rejoin-{}", std::process::id());

	// The owner runs on its own runtime, so dropping that runtime closes its
	// listener exactly as the owning process exiting would.
	let (ready_tx, ready_rx) = std::sync::mpsc::channel();
	let (exit_tx, exit_rx) = std::sync::mpsc::channel::<()>();
	let owner_dir = dir.path().to_path_buf();
	let owner_key = key.clone();
	let owner = std::thread::spawn(move || {
		let rt = tokio::runtime::Runtime::new().unwrap();
		let provider = rt.block_on(join_in(&owner_dir, &owner_key, fake_build())).unwrap();
		assert!(rt.block_on(provider.owns_model()));
		ready_tx.send(()).unwrap();
		exit_rx.recv().ok();
		drop(provider);
		drop(rt);
	});
	ready_rx.recv().unwrap();

	let client = join_in(dir.path(), &key, fake_build()).await.expect("client attaches");
	assert!(!client.owns_model().await);
	assert_eq!(
		client.generate_embedding("ab").await.expect("remote embed").0,
		vec![2.0, 1.0]
	);

	exit_tx.send(()).unwrap();
	owner.join().unwrap();

	let (vector, _) = client
		.generate_embedding("abcd")
		.await
		.expect("answered after the owner exited");
	assert_eq!(vector, vec![4.0, 1.0]);
	assert!(
		client.owns_model().await,
		"the client won the re-election and serves the weights now"
	);
	let (vectors, _) = client
		.generate_embeddings_batch(vec!["abc".to_string()], InputType::Document)
		.await
		.expect("batch after re-election");
	assert_eq!(vectors, vec![vec![3.0, 2.0]]);

	// A process that joins now attaches to the new owner instead of loading a copy.
	let late = join_in(dir.path(), &key, fake_build()).await.expect("late joiner");
	assert!(!late.owns_model().await);
	assert_eq!(
		late.generate_embedding("abcde").await.expect("via the new owner").0,
		vec![5.0, 1.0]
	);
	let current = read_endpoint(&endpoint_path(dir.path(), &key).unwrap(), &key).unwrap();
	assert_eq!(current.pid, std::process::id());
}
