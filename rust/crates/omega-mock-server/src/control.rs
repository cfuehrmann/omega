//! Tiny HTTP control API exposed alongside the main server on a separate
//! port.  Tests use it to (a) wait for readiness, (b) drain the captured
//! LLM call history, and (c) reset that history between tests.
//!
//! Routes (all on `--ctrl-port`, default 3004):
//!
//! | Method  | Path                    | Behaviour                          |
//! |---------|-------------------------|------------------------------------|
//! | `GET`   | `/control/ready`        | `200 "ok"` once the server is up.  |
//! | `GET`   | `/control/llm-calls`    | JSON array of [`CapturedCall`].    |
//! | `POST`  | `/control/reset-calls`  | Clear the history; `200 "ok"`.     |
//!
//! Response shapes match the historical TypeScript fixture exactly so the
//! Playwright test code does not need to change.

use axum::{Json, Router, extract::State, routing::get, routing::post};

use crate::provider::{CallHistory, CapturedCall};

pub fn router(history: CallHistory) -> Router {
    Router::new()
        .route("/control/ready", get(ready))
        .route("/control/llm-calls", get(llm_calls))
        .route("/control/reset-calls", post(reset_calls))
        .with_state(history)
}

async fn ready() -> &'static str {
    "ok"
}

async fn llm_calls(State(history): State<CallHistory>) -> Json<Vec<CapturedCall>> {
    Json(history.snapshot())
}

async fn reset_calls(State(history): State<CallHistory>) -> &'static str {
    history.reset();
    "ok"
}
