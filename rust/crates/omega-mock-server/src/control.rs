//! Tiny HTTP control API exposed alongside the main server on a separate
//! port. Tests use it to (a) wait for readiness, (b) drain the captured
//! LLM call history, (c) reset that history between tests, and (d) load
//! a per-test script of mock responses for the internal Anthropic-shaped
//! SSE fake.
//!
//! The fake itself, the `MockResponse` enum, and the `CallHistory` /
//! `Script` machinery all live in the workspace's `omega-test-fixtures`
//! crate — see that crate's docs for the wire shapes.
//!
//! Routes (all on `--ctrl-port`, default 3004):
//!
//! | Method | Path                   | Behaviour                          |
//! |--------|------------------------|------------------------------------|
//! | `GET`  | `/control/ready`       | `200 "ok"` once the server is up.  |
//! | `GET`  | `/control/llm-calls`   | JSON array of `CapturedCall`.      |
//! | `POST` | `/control/reset-calls` | Clear the history; `200 "ok"`.     |
//! | `POST` | `/control/script`      | Replace the queue with a JSON      |
//! |        |                        | array of `MockResponse`; `"ok"`.   |

use axum::{Json, Router, extract::State, routing::get, routing::post};
use omega_test_fixtures::{CallHistory, CapturedCall, MockResponse, Script};

#[derive(Clone)]
struct ControlState {
    history: CallHistory,
    script: Script,
}

pub fn router(history: CallHistory, script: Script) -> Router {
    Router::new()
        .route("/control/ready", get(ready))
        .route("/control/llm-calls", get(llm_calls))
        .route("/control/reset-calls", post(reset_calls))
        .route("/control/script", post(set_script))
        .with_state(ControlState { history, script })
}

async fn ready() -> &'static str {
    "ok"
}

async fn llm_calls(State(s): State<ControlState>) -> Json<Vec<CapturedCall>> {
    Json(s.history.snapshot())
}

async fn reset_calls(State(s): State<ControlState>) -> &'static str {
    s.history.reset();
    "ok"
}

async fn set_script(
    State(s): State<ControlState>,
    Json(steps): Json<Vec<MockResponse>>,
) -> &'static str {
    if let Ok(mut q) = s.script.lock() {
        q.clear();
        q.extend(steps);
    }
    "ok"
}
