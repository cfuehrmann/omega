//! `find_files` — find files/directories by name/glob using `fd` (preferred)
//! or `find` as a fallback.

use std::fmt::Write as _;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::grep_files::{has_command, run_subprocess};

const DEFAULT_MAX_RESULTS: usize = 200;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("find_files: pattern is required")?;
    let path = input["path"]
        .as_str()
        .ok_or("find_files: path is required")?;
    let type_filter = input["type"].as_str();
    let hidden = input["hidden"].as_bool().unwrap_or(false);
    let max_results = input["max_results"]
        .as_u64()
        .map_or(DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(DEFAULT_MAX_RESULTS)
        });

    let (cmd, args) = if has_command("fd").await {
        let mut a: Vec<String> = vec!["--glob".into(), pattern.into(), path.into()];
        if let Some(t) = type_filter {
            a.push("--type".into());
            a.push(t.into());
        }
        if hidden {
            a.push("--hidden".into());
            a.push("--no-ignore".into());
        }
        ("fd", a)
    } else {
        let mut a: Vec<String> = vec![path.into()];
        if let Some(t) = type_filter {
            a.push("-type".into());
            a.push(t.into());
        }
        a.push("-name".into());
        a.push(pattern.into());
        if !hidden {
            a.extend(["!".into(), "-name".into(), ".*".into()]);
        }
        ("find", a)
    };

    let out = run_subprocess(cmd, &args).await?;

    if out.code != 0 && out.code != 1 {
        let msg = if out.stderr.trim().is_empty() {
            format!("find_files: command failed (exit {})", out.code)
        } else {
            out.stderr.trim().to_owned()
        };
        return Err(msg);
    }

    let raw = out.stdout.trim();
    if raw.is_empty() {
        return Ok("No files found.".to_owned());
    }

    let mut lines: Vec<&str> = raw.split('\n').collect();
    let truncated = lines.len() > max_results;
    if truncated {
        lines.truncate(max_results);
    }

    let mut result = lines.join("\n");
    if truncated {
        // `write!` to a String is infallible; the `Err` variant is unreachable.
        let _ = write!(result, "\n\n[Truncated at {max_results} results]");
    }
    Ok(result)
}
