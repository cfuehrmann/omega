//! `edit_file` tool \u2014 stub body for Phase 1d.0a.
//!
//! Real implementation lands in Phase 1d.0b. The signature here is the
//! stable contract: `Result<String, String>` where `Ok` is the content
//! sent back to the LLM and `Err` is converted to `is_error: true`.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub async fn execute(_input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    Err("edit_file: not yet implemented (Phase 1d.0b)".into())
}
