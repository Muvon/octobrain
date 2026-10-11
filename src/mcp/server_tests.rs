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
use rmcp::model::{ClientCapabilities, ClientConfig, RequestId};
use serde_json::json;

fn test_config() -> Config {
    toml::from_str(include_str!("../../config-templates/default.toml")).unwrap()
}

fn serve_in_memory(
    server: &McpServer,
    client: Option<ClientConfig>,
) -> (
    rmcp::service::RunningService<RoleServer, McpServer>,
    tokio::io::DuplexStream,
) {
    server.box_sync_started.store(true, Ordering::SeqCst);
    let (transport, client_transport) = tokio::io::duplex(4096);
    (
        rmcp::service::serve_directly(server.clone(), transport, client),
        client_transport,
    )
}

fn context(
    peer: &rmcp::Peer<RoleServer>,
    session: serde_json::Value,
) -> RequestContext<RoleServer> {
    let mut context = RequestContext::new(RequestId::Number(1), peer.clone());
    let capabilities: ClientCapabilities =
        serde_json::from_value(json!({"experimental": {"session": session}})).unwrap();
    context.meta.set_client_capabilities(capabilities);
    context
}

/// Each named octo works in its own directory under `root`; `project` is octomind's
/// hashed id and must not decide the memory scope.
fn session(name: &str, root: &std::path::Path) -> serde_json::Value {
    let workdir = root.join(name);
    std::fs::create_dir_all(&workdir).unwrap();
    json!({"project": "0123456789abcdef", "workdir": workdir, "git": false, "role": "cloud", "session_id": name})
}

fn scope(name: &str, root: &std::path::Path) -> String {
    octolib::utils::path_to_scope(&root.join(name))
}

#[tokio::test]
async fn non_git_session_locks_scope_with_local_repositories() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("unrelated/.git")).unwrap();
    let workdir = root.path().join("plain-workspace");
    std::fs::create_dir(&workdir).unwrap();
    assert!(!workdir.join(".git").exists());
    let server = McpServer::new(test_config(), root.path().to_path_buf());
    assert_eq!(
        server
            .workdir_scopes
            .lock()
            .await
            .get(root.path())
            .map(|cached| !cached.projects.is_empty()),
        Some(true)
    );
    let (service, _client) = serve_in_memory(&server, None);
    let state = server
        .ensure_session_context(&context(
            service.peer(),
            session("plain-workspace", root.path()),
        ))
        .await
        .unwrap();
    assert!(state.scope_locked);
    assert_eq!(
        state.scope.as_deref(),
        Some(scope("plain-workspace", root.path()).as_str())
    );
    assert_eq!(state.role.as_deref(), Some("cloud"));
    let mut standalone = session("plain-workspace", root.path());
    standalone.as_object_mut().unwrap().remove("workdir");
    let state = server
        .ensure_session_context(&context(service.peer(), standalone))
        .await
        .unwrap();
    assert!(!state.scope_locked);
    let state = server
        .ensure_session_context(&RequestContext::new(
            RequestId::Number(2),
            service.peer().clone(),
        ))
        .await
        .unwrap();
    assert!(!state.scope_locked);
    assert!(!state.role_locked);
}

#[tokio::test]
async fn older_octomind_session_metadata_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let server = McpServer::new(test_config(), root.path().to_path_buf());
    let (service, _client) = serve_in_memory(&server, None);
    let mut metadata = session("valid", root.path());
    metadata["workdir"] = json!(42);
    assert!(server
        .ensure_session_context(&context(service.peer(), metadata))
        .await
        .is_err());
    // Octomind 0.57.1 and older: no anchor, so empty `project` and `workdir`.
    let mut metadata = session("valid", root.path());
    metadata["project"] = json!("");
    metadata["workdir"] = json!("");
    let state = server
        .ensure_session_context(&context(service.peer(), metadata))
        .await
        .unwrap();
    assert_eq!(
        state.scope.as_deref(),
        Some(octolib::utils::path_to_scope(root.path()).as_str())
    );
    let mut metadata = session("valid", root.path());
    metadata.as_object_mut().unwrap().remove("project");
    let state = server
        .ensure_session_context(&context(service.peer(), metadata))
        .await
        .unwrap();
    assert_eq!(
        state.scope.as_deref(),
        Some(scope("valid", root.path()).as_str())
    );
}

#[tokio::test]
async fn shared_server_scopes_writes_reads_and_forget() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("memory");
    let server = McpServer::new(test_config(), root.path().to_path_buf());
    let (service, _client) = serve_in_memory(&server, None);
    for (name, role) in [
        ("", "cloud"),
        ("alpha", "cloud"),
        ("beta", "cloud"),
        ("alpha", "reviewer"),
    ] {
        let scope = if name.is_empty() {
            String::new()
        } else {
            scope(name, root.path())
        };
        let key = (Some(scope), Some(role.to_string()));
        let provider =
            MemoryProvider::for_scope_test(&db, root.path(), key.0.clone(), key.1.clone()).await;
        server.memory.lock().await.insert(key, provider);
    }
    let global = server
        .memory
        .lock()
        .await
        .get(&(Some(String::new()), Some("cloud".to_string())))
        .unwrap()
        .clone();
    global
        .execute_memorize(&json!({"title": "legacy-shared", "content": "legacy shared fact"}))
        .await
        .unwrap();
    let mut memory_ids = HashMap::new();
    for (scope, title) in [
        ("alpha", "alpha-private"),
        ("beta", "beta-private"),
        ("alpha", "alpha-again"),
    ] {
        let params = serde_json::from_value(
            json!({"title": title, "content": title, "scope": "ignored", "role": "ignored"}),
        )
        .unwrap();
        let result = server
            .memorize(
                context(service.peer(), session(scope, root.path())),
                Parameters(params),
            )
            .await
            .unwrap();
        memory_ids.insert(
            title,
            result.strip_prefix("Memory stored: ").unwrap().to_string(),
        );
    }
    let mut reviewer = session("alpha", root.path());
    reviewer["role"] = json!("reviewer");
    server
        .memorize(
            context(service.peer(), reviewer),
            Parameters(
                serde_json::from_value(
                    json!({"title": "reviewer-private", "content": "reviewer private fact"}),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    let tools = server
        .list_tools(None, context(service.peer(), session("alpha", root.path())))
        .await
        .unwrap();
    let memorize = tools
        .tools
        .iter()
        .find(|tool| tool.name == "memorize")
        .unwrap();
    assert!(memorize.input_schema["properties"].get("global").is_some());
    server.memorize(
        context(service.peer(), session("alpha", root.path())),
        Parameters(serde_json::from_value(json!({"title": "explicit-global", "content": "account-wide fact", "global": true})).unwrap()),
    ).await.unwrap();
    let connection = lancedb::connect(db.to_str().unwrap())
        .execute()
        .await
        .unwrap();
    let table = connection.open_table("memories").execute().await.unwrap();
    for (name, role, count) in [
        ("", "cloud", 2),
        ("alpha", "cloud", 2),
        ("beta", "cloud", 1),
        ("alpha", "reviewer", 1),
    ] {
        let scope = if name.is_empty() {
            String::new()
        } else {
            scope(name, root.path())
        };
        assert_eq!(
            table
                .count_rows(Some(format!("scope = '{}' AND role = '{}'", scope, role)))
                .await
                .unwrap(),
            count
        );
    }
    let recall = async |scope| {
        server
            .remember(
                context(service.peer(), session(scope, root.path())),
                Parameters(
                    serde_json::from_value(
                        json!({"query": "fact", "min_relevance": 0.0, "limit": 50}),
                    )
                    .unwrap(),
                ),
            )
            .await
            .unwrap()
    };
    let (alpha, beta) = tokio::join!(recall("alpha"), recall("beta"));
    assert!(
        alpha.contains("alpha-private")
            && alpha.contains("alpha-again")
            && alpha.contains("legacy-shared")
            && alpha.contains("explicit-global"),
        "{alpha}"
    );
    assert!(
        !alpha.contains("beta-private") && !alpha.contains("reviewer-private"),
        "{alpha}"
    );
    assert!(
        beta.contains("beta-private")
            && beta.contains("legacy-shared")
            && beta.contains("explicit-global"),
        "{beta}"
    );
    assert!(
        !beta.contains("alpha-private") && !beta.contains("alpha-again"),
        "{beta}"
    );
    let global_result = global
        .execute_remember(&json!({"query": "fact", "min_relevance": 0.0, "limit": 50}))
        .await
        .unwrap();
    assert!(global_result.contains("legacy-shared") && global_result.contains("explicit-global"));
    assert!(
        !global_result.contains("alpha-private") && !global_result.contains("beta-private"),
        "{global_result}"
    );
    server
        .forget(
            context(service.peer(), session("alpha", root.path())),
            Parameters(
                serde_json::from_value(
                    json!({"memory_id": memory_ids["beta-private"], "confirm": true}),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    assert!(recall("beta").await.contains("beta-private"));
    server
        .forget(
            context(service.peer(), session("alpha", root.path())),
            Parameters(serde_json::from_value(json!({"query": "fact", "confirm": true})).unwrap()),
        )
        .await
        .unwrap();
    let after_alpha = recall("alpha").await;
    let after_beta = recall("beta").await;
    assert!(
        !after_alpha.contains("alpha-private") && !after_alpha.contains("alpha-again"),
        "{after_alpha}"
    );
    assert!(
        after_alpha.contains("legacy-shared") && after_alpha.contains("explicit-global"),
        "{after_alpha}"
    );
    assert!(after_beta.contains("beta-private"), "{after_beta}");
}

#[tokio::test]
async fn request_context_overrides_legacy_handshake() {
    let root = tempfile::tempdir().unwrap();
    let server = McpServer::new(test_config(), root.path().to_path_buf());
    let capabilities = serde_json::from_value(json!({"experimental": {"session": {
        "project": "legacy", "role": "cloud", "workdir": root.path(), "git": false
    }}}))
    .unwrap();
    let info = ClientConfig::new(capabilities, Implementation::new("octomind", "old"))
        .with_protocol_version(ProtocolVersion::V_2025_03_26);
    let (service, _client) = serve_in_memory(&server, Some(info));
    let legacy = octolib::utils::path_to_scope(root.path());
    let handshake = RequestContext::new(RequestId::Number(1), service.peer().clone());
    let state = server.ensure_session_context(&handshake).await.unwrap();
    assert_eq!(state.scope.as_deref(), Some(legacy.as_str()));
    let state = server
        .ensure_session_context(&context(service.peer(), session("current", root.path())))
        .await
        .unwrap();
    assert_eq!(
        state.scope.as_deref(),
        Some(scope("current", root.path()).as_str())
    );
    let state = server.ensure_session_context(&handshake).await.unwrap();
    assert_eq!(state.scope.as_deref(), Some(legacy.as_str()));
}

#[tokio::test]
async fn multi_repo_session_workdir_stays_unlocked_and_discovery_is_cached() {
    let root = tempfile::tempdir().unwrap();
    let workdir = root.path().join("projects");
    std::fs::create_dir_all(workdir.join("one/.git")).unwrap();
    std::fs::create_dir_all(workdir.join("two/.git")).unwrap();
    let server = McpServer::new(test_config(), root.path().to_path_buf());
    assert_eq!(
        server
            .workdir_scopes
            .lock()
            .await
            .get(root.path())
            .map(|cached| !cached.projects.is_empty()),
        Some(false)
    );
    let (service, _client) = serve_in_memory(&server, None);
    let metadata = session("projects", root.path());
    let state = server
        .ensure_session_context(&context(service.peer(), metadata.clone()))
        .await
        .unwrap();
    assert!(!state.scope_locked);
    assert_eq!(state.scope, None);
    assert_eq!(state.role.as_deref(), Some("cloud"));
    assert_eq!(
        server
            .workdir_scopes
            .lock()
            .await
            .get(&workdir)
            .map(|cached| !cached.projects.is_empty()),
        Some(true)
    );
    // The session picks among its own repos and never runs unscoped.
    let own = scope("projects", root.path());
    let one = octolib::utils::path_to_scope(&workdir.join("one"));
    assert_eq!(state.default_scope.as_deref(), Some(own.as_str()));
    assert!(state.scope_choices.contains(&one));
    assert_eq!(state.resolve_scope(None).unwrap(), Some(own.clone()));
    assert_eq!(
        state.resolve_scope(Some(one.clone())).unwrap(),
        Some(one.clone())
    );
    assert_eq!(
        state.resolve_scope(Some(String::new())).unwrap(),
        Some(String::new())
    );
    let error = state
        .resolve_scope(Some("github.com/other/repo".to_string()))
        .unwrap_err();
    assert!(error.message.contains(&one), "{}", error.message);
    let tools = server
        .list_tools(None, context(service.peer(), metadata.clone()))
        .await
        .unwrap();
    let remember = tools
        .tools
        .iter()
        .find(|tool| tool.name == "remember")
        .unwrap();
    assert_eq!(
        remember.input_schema["properties"]["scope"]["enum"],
        json!(state.scope_options())
    );
    std::fs::remove_dir(workdir.join("one/.git")).unwrap();
    std::fs::remove_dir(workdir.join("two/.git")).unwrap();
    let state = server
        .ensure_session_context(&context(service.peer(), metadata.clone()))
        .await
        .unwrap();
    assert!(!state.scope_locked, "discovery is cached for this workdir");
    let mut git_root = metadata;
    git_root["git"] = json!(true);
    let state = server
        .ensure_session_context(&context(service.peer(), git_root))
        .await
        .unwrap();
    assert!(state.scope_locked);
    assert_eq!(
        state.scope.as_deref(),
        Some(scope("projects", root.path()).as_str())
    );
    let mut no_workdir = session("projects", root.path());
    no_workdir.as_object_mut().unwrap().remove("workdir");
    let state = server
        .ensure_session_context(&context(service.peer(), no_workdir))
        .await
        .unwrap();
    assert!(state.scope_locked);
    assert_eq!(
        state.scope.as_deref(),
        Some(octolib::utils::path_to_scope(root.path()).as_str())
    );
}
