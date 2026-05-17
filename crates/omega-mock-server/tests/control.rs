//! Integration tests for the `omega-mock-server` control HTTP API.
//!
//! Drives the control router directly via `tower::ServiceExt::oneshot`
//! — no TCP binding, no real LLM provider. Kills four mutation
//! survivors at once:
//!  * `omega-mock-server::control::ready -> &str` (×2)
//!  * `omega-test-fixtures::CallHistory::snapshot` (×N)
//!  * `omega-test-fixtures::CallHistory::reset`
//!
//! These are the routes the new Phase-4 `chromiumoxide` browser
//! harness will use to script per-test LLM responses and inspect what
//! the agent actually sent. Without these tests the routes would only
//! be exercised by deleted Playwright code.

#![allow(clippy::unwrap_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use omega_mock_server::control;
use omega_test_fixtures::{CallHistory, CapturedCall, CapturedMessage, new_script};
use tower::ServiceExt;

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn fake_call(content: &str) -> CapturedCall {
    CapturedCall {
        system_kind: "task",
        at: 0,
        messages: vec![CapturedMessage {
            role: "user".into(),
            content: content.into(),
        }],
    }
}

/// Kills `replace ready -> &'static str with ""` and `with "xyzzy"`.
/// `/control/ready` is the readiness probe consumed by the new
/// chromiumoxide harness; the body must be exactly "ok".
#[tokio::test]
async fn ready_returns_literal_ok() {
    let app = control::router(CallHistory::new(), new_script());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/control/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");
}

/// Kills `replace CallHistory::snapshot -> Vec<CapturedCall> with vec![]`.
/// The control router exposes snapshot via `GET /control/llm-calls`;
/// the harness uses it to assert what the agent sent to the LLM. If
/// snapshot returned an empty vec regardless of state, the harness
/// would silently pass on every assertion-by-history.
#[tokio::test]
async fn llm_calls_reflects_recorded_history() {
    let history = CallHistory::new();
    let app = control::router(history.clone(), new_script());

    // Empty before any push.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/control/llm-calls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let calls: Vec<serde_json::Value> = serde_json::from_str(&body_string(resp).await).unwrap();
    assert!(
        calls.is_empty(),
        "fresh history must surface as empty array"
    );

    // Record two calls via the public push API; snapshot must surface
    // *exactly* those two, in order, with the recorded content. A
    // mutation that returns `vec![]` or `vec![Default::default()]`
    // fails both the length and content assertions below.
    history.push(fake_call("first"));
    history.push(fake_call("second"));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/control/llm-calls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let calls: Vec<serde_json::Value> = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(calls.len(), 2, "snapshot must surface both recorded calls");
    assert_eq!(calls[0]["messages"][0]["content"], "first");
    assert_eq!(calls[1]["messages"][0]["content"], "second");
}

/// Kills `replace CallHistory::reset` mutants. After `POST
/// /control/reset-calls` the snapshot must be empty again. The harness
/// calls reset between tests; if reset were a no-op, leaked state from
/// one test would silently corrupt the next.
#[tokio::test]
async fn reset_calls_clears_history() {
    let history = CallHistory::new();
    let app = control::router(history.clone(), new_script());
    history.push(fake_call("leak"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/reset-calls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/control/llm-calls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let calls: Vec<serde_json::Value> = serde_json::from_str(&body_string(resp).await).unwrap();
    assert!(
        calls.is_empty(),
        "after reset, snapshot must be empty (got {calls:?})"
    );
}
