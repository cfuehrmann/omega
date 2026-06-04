//! `stop_monitor(id)` — stop an async monitor.
//!
//! Returns **immediately**.  Kills the monitor's whole process tree (no
//! orphans) and emits a `MonitorStopped` event with reason `AgentStopped`
//! via `extra_events` (single-writer rule — the agent loop records it).
//!
//! Stopping a **dead or unknown** monitor is a **no-op**, not an error: the
//! tool returns success and emits no event (the monitor is already gone, so
//! there is nothing to project and nothing for the agent to learn).

use serde_json::Value;

use omega_types::events::{MonitorStopReason, MonitorStoppedEvent, OmegaEvent};

use crate::ToolResult;
use crate::monitors::now_iso;
use crate::tool_ctx::ToolCtx;

/// Execute the `stop_monitor` tool.  Returns immediately (killing the process
/// tree is synchronous and fast).
pub fn execute(input: &Value, ctx: Option<&ToolCtx>) -> ToolResult {
    let Some(ctx) = ctx else {
        return ToolResult::err("stop_monitor: no session context — monitors are not available");
    };
    let Some(manager) = &ctx.monitors else {
        return ToolResult::err("stop_monitor: monitors are not enabled for this session");
    };
    let Some(id) = input["id"].as_str() else {
        return ToolResult::err("stop_monitor: missing 'id' field");
    };

    if manager.stop(id) {
        let mut result = ToolResult::ok(format!("Monitor `{id}` stopped."));
        result
            .extra_events
            .push(OmegaEvent::MonitorStopped(MonitorStoppedEvent {
                id: id.to_owned(),
                reason: MonitorStopReason::AgentStopped,
                exit_code: None,
                time: now_iso(),
            }));
        result
    } else {
        // No-op: unknown id or already-stopped monitor. Not an error.
        ToolResult::ok(format!(
            "Monitor `{id}` is not running (already stopped or unknown) — no action taken."
        ))
    }
}
