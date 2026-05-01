//! `grep_files` — search for a pattern across files using `rg` (preferred)
//! or `grep` as a fallback.

use std::process::Stdio;

use serde_json::Value;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const DEFAULT_CONTEXT: u64 = 2;
const DEFAULT_MAX_RESULTS: usize = 200;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("grep_files: pattern is required")?;
    let path = input["path"]
        .as_str()
        .ok_or("grep_files: path is required")?;
    let file_glob = input["file_glob"].as_str();
    let context_lines = input["context_lines"].as_u64().unwrap_or(DEFAULT_CONTEXT);
    let case_sensitive = input["case_sensitive"].as_bool().unwrap_or(false);
    let max_results = input["max_results"]
        .as_u64()
        .map_or(DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(DEFAULT_MAX_RESULTS)
        });

    let (cmd, args) = if has_command("rg").await {
        let mut a: Vec<String> = vec![
            "--line-number".into(),
            "--with-filename".into(),
            "--no-heading".into(),
        ];
        if case_sensitive {
            a.push("--case-sensitive".into());
        } else {
            a.push("--ignore-case".into());
        }
        if let Some(glob) = file_glob {
            a.push("--glob".into());
            a.push(glob.into());
        }
        if context_lines > 0 {
            a.push("--context".into());
            a.push(context_lines.to_string());
        }
        a.push("--".into());
        a.push(pattern.into());
        a.push(path.into());
        ("rg", a)
    } else {
        let mut a: Vec<String> = vec!["-rn".into()];
        if !case_sensitive {
            a.push("-i".into());
        }
        if let Some(glob) = file_glob {
            a.push(format!("--include={glob}"));
        }
        if context_lines > 0 {
            a.push(format!("-C{context_lines}"));
        }
        a.push("--".into());
        a.push(pattern.into());
        a.push(path.into());
        ("grep", a)
    };

    let out = run_subprocess(cmd, &args).await?;

    // exit code 1 means "no matches" — not an error.
    if out.code != 0 && out.code != 1 {
        let msg = if out.stderr.trim().is_empty() {
            format!("Search failed (exit {})", out.code)
        } else {
            out.stderr.trim().to_owned()
        };
        return Err(msg);
    }

    let raw = out.stdout.trim();
    if raw.is_empty() {
        return Ok("No matches found.".to_owned());
    }

    let lines: Vec<&str> = raw.split('\n').collect();
    if lines.len() <= max_results {
        return Ok(lines.join("\n"));
    }

    Ok(format!(
        "{}\n\n[truncated: showing {max_results} of {} matches]",
        lines[..max_results].join("\n"),
        lines.len()
    ))
}

// ---------------------------------------------------------------------------
// Shared helpers (used by find_files and fetch_url too)
// ---------------------------------------------------------------------------

pub(crate) struct SubprocOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

pub(crate) async fn run_subprocess(
    cmd: &str,
    args: &[String],
) -> Result<SubprocOutput, String> {
    let output = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{cmd}: {e}"))?;

    Ok(SubprocOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

pub(crate) async fn has_command(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}
