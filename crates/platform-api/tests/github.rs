//! Repository preview and confirmed-action calls against a synthetic Platform gateway.

#![allow(clippy::expect_used, clippy::panic, reason = "test assertions")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{Json, Router, body::Bytes, http::HeaderMap, routing::post};
use ratatoskr_github_contracts::{
    ConfirmationEvidenceRef, GitHubRepositoryUrl, RepositoryActionCapability,
    RepositoryActionIdempotencyKey, RepositoryActionRequest, RepositoryPreviewRequest,
};
use serde_json::{Value, json};
use url::Url;

#[tokio::test]
async fn preview_and_action_calls_use_platform_session_and_shared_contract() {
    let seen = Arc::new(Mutex::new(Vec::<(String, String, Value)>::new()));
    let log = Arc::clone(&seen);
    let app = Router::new().route(
        "/v1/gh/repositories/{kind}",
        post(move |axum::extract::Path(kind): axum::extract::Path<String>, headers: HeaderMap, body: Bytes| {
            let log = Arc::clone(&log);
            async move {
                let authorization = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
                let value: Value = serde_json::from_slice(&body).expect("contract JSON");
                log.lock().expect("log").push((kind.clone(), authorization, value));
                if kind == "preview" {
                    Json(json!({
                        "target": {"github_repository_numeric_id": 42, "repository_full_name": "owner/repository", "canonical_url": "https://github.com/owner/repository"},
                        "description": "A tool", "stargazer_count": 7, "primary_language": "Rust",
                        "available_actions": ["metadata", "track"]
                    }))
                } else {
                    Json(json!({
                        "aggregate": "succeeded", "metadata": {"status": "succeeded"},
                        "provider_star": {"status": "skipped", "reason": "not_applicable"},
                        "desired_backup": {"status": "skipped", "reason": "not_applicable"}
                    }))
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
    let client = platform_api::Client::new(
        &Url::parse(&format!("http://{address}")).expect("url"),
        Duration::from_secs(2),
    )
    .expect("client");
    let preview = client
        .preview_repository(
            "session",
            &RepositoryPreviewRequest {
                repository_url: GitHubRepositoryUrl::parse("https://github.com/owner/repository")
                    .expect("url"),
            },
        )
        .await
        .expect("preview");
    let action = RepositoryActionRequest::new(
        RepositoryActionCapability::Metadata,
        preview.target,
        None,
        ConfirmationEvidenceRef::parse("telegram-confirmation:flow-1").expect("evidence"),
        RepositoryActionIdempotencyKey::parse("telegram-github-action.flow-1").expect("key"),
    )
    .expect("action");
    client
        .apply_repository_action("session", &action)
        .await
        .expect("result");

    let calls = seen.lock().expect("log");
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|(_, bearer, _)| bearer == "Bearer session")
    );
    assert_eq!(calls[0].0, "preview");
    assert_eq!(calls[1].0, "actions");
}
