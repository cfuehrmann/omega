//! `monitor(description, command)` — start an async monitor.
//!
//! Spawns `command` as a background OS process and returns **immediately**
//! with a monitor id.  The process's stdout lines flow asynchronously into
//! the session's pending queue (delivered to the agent at a boundary in
//! Phase 2); stderr is captured diagnostically.  A `MonitorStarted` event is
//! emitted via `extra_events` so the single writer (the agent loop) records
//! it — the tool never touches `events.jsonl` itself (design §4).

use serde_json::Value;

use omega_types::events::{MonitorStartedEvent, OmegaEvent};

use crate::ToolResult;
use crate::tool_ctx::ToolCtx;

/// Execute the `monitor` tool.  Returns immediately; never blocks on process
/// output (spawning is synchronous; the process runs in detached background
/// tasks).
pub fn execute(input: &Value, ctx: Option<&ToolCtx>) -> ToolResult {
    let Some(ctx) = ctx else {
        return ToolResult::err("monitor: no session context — monitors are not available");
    };
    let Some(manager) = &ctx.monitors else {
        return ToolResult::err("monitor: monitors are not enabled for this session");
    };
    let Some(description) = input["description"].as_str() else {
        return ToolResult::err("monitor: missing 'description' field");
    };
    let Some(command) = input["command"].as_str() else {
        return ToolResult::err("monitor: missing 'command' field");
    };

    match manager.spawn(description, command) {
        Ok(spawned) => {
            let mut result = ToolResult::ok(format!(
                "Monitor started with id `{id}`. It runs asynchronously: its \
                 stdout lines arrive later as injected messages at the next \
                 boundary. Stop it with stop_monitor(\"{id}\") when no longer \
                 needed.",
                id = spawned.id,
            ));
            result
                .extra_events
                .push(OmegaEvent::MonitorStarted(MonitorStartedEvent {
                    id: spawned.id,
                    description: description.to_owned(),
                    command: command.to_owned(),
                    time: spawned.started_at,
                }));
            result
        }
        Err(e) => ToolResult::err(e),
    }
}
