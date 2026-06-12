//! `write_stdin` — write text to the stdin of a monitor or background job.
//!
//! The handle is the monitor/job **id** (the same id returned by
//! `run_background` and `monitor`, and shown in the roster), not an OS pid.
//! Both streaming monitors and background jobs are spawned with a piped
//! stdin, so this tool feeds either kind through the same
//! [`MonitorManager`](crate::monitors::MonitorManager) path.

use serde_json::Value;

use crate::tool_ctx::ToolCtx;

pub async fn execute(input: &Value, ctx: Option<&ToolCtx>) -> Result<String, String> {
    let ctx = ctx.ok_or("write_stdin: no session context — monitors are not available")?;
    let manager = ctx
        .monitors
        .as_ref()
        .ok_or("write_stdin: monitors are not enabled for this session")?;
    let id = input["id"]
        .as_str()
        .ok_or("write_stdin: 'id' is required")?;
    let text = input["text"]
        .as_str()
        .ok_or("write_stdin: 'text' is required")?;
    let end_stdin = input["end_stdin"].as_bool().unwrap_or(false);

    let char_count = text.chars().count();

    manager.write_stdin(id, text.as_bytes()).await?;

    if end_stdin {
        manager.close_stdin(id).await?;
        return Ok(format!(
            "Wrote {char_count} chars to stdin of `{id}` and closed stdin (EOF)"
        ));
    }

    Ok(format!("Wrote {char_count} chars to stdin of `{id}`"))
}
