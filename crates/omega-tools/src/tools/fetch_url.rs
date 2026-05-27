//! `fetch_url` — download a URL to a content-addressed cache file, then run
//! a postprocess shell command on it.
//!
//! When a session context is provided the cache is placed under
//! `<ctx.cache_dir>/fetch/<url-hash>.txt`; otherwise a per-process temp
//! directory is used for test compatibility.  Cross-session deduplication
//! is intentionally dropped (see `backlog/tee-on-truncate.md`).

use std::fmt::Write as _;

use crate::cap_and_tee::{TruncationBias, cap_and_tee};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;

use crate::tool_ctx::ToolCtx;

// ---------------------------------------------------------------------------
// Subprocess helper (used only by this module for the postprocess call)
// ---------------------------------------------------------------------------

struct SubprocOutput {
    stdout: String,
    stderr: String,
    /// `Some(code)` for a normal exit; `None` when killed by a signal.
    /// Modelled as `Option` rather than a magic `-1` sentinel so the two
    /// conditions never collide with a legitimate shell exit code.
    code: Option<i32>,
}

async fn run_subprocess(cmd: &str, args: &[String]) -> Result<SubprocOutput, String> {
    use std::process::Stdio;
    use tokio::process::Command;

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
        code: output.status.code(),
    })
}

/// LLM-facing cap on the postprocess output; the full output is always tee'd to disk.
/// Deliberately smaller than `run_command`'s cap: postprocess is a targeted extraction
/// step (`grep`, `head -N`, `jq`) and should produce compact output by design.
const PP_CAP: usize = 16_000;
const FETCH_TIMEOUT_S: u64 = 15;

/// Maximum number of lines returned in shell-gated mode (no postprocess).
const SHELL_GATED_MAX_LINES: usize = 2000;
/// Maximum number of bytes returned in shell-gated mode (no postprocess).
const SHELL_GATED_MAX_BYTES: usize = 50 * 1024;

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

fn cache_dir() -> &'static PathBuf {
    CACHE_DIR
        .get_or_init(|| std::env::temp_dir().join(format!("omega-webcache-{}", std::process::id())))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
    ctx: Option<&ToolCtx>,
) -> Result<String, String> {
    // Check whether the shell-gated mode is active: when no shell-execution
    // tool is in this session's tool_selection, the postprocess pipeline is
    // disabled to close the shell-loophole.
    let shell_gated = ctx.is_some_and(|c| {
        !c.tool_selection.iter().any(|n| {
            matches!(
                n.as_str(),
                "run_command" | "run_background" | "wait_for_output" | "write_stdin"
            )
        })
    });

    let url_str = input["url"]
        .as_str()
        .ok_or("fetch_url: url is required")?
        .trim()
        .to_owned();

    if url_str.is_empty() {
        return Err("fetch_url: url must not be empty".into());
    }

    if shell_gated {
        // Defensive: if the caller somehow included a postprocess value despite
        // the schema not listing it, log a warning and ignore it.
        if input["postprocess"].as_str().is_some() {
            eprintln!(
                "warning: fetch_url received a postprocess value while \
                 shell-gated mode is active — ignoring it (shell loophole closed)"
            );
        }
        return execute_shell_gated(url_str, ctx).await;
    }

    // Default mode: postprocess is required.
    let postprocess = input["postprocess"]
        .as_str()
        .ok_or("fetch_url: postprocess is required")?
        .trim()
        .to_owned();

    if postprocess.is_empty() {
        return Err("fetch_url: postprocess must not be empty".into());
    }

    let parsed =
        reqwest::Url::parse(&url_str).map_err(|_| format!("fetch_url: invalid URL: {url_str}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "fetch_url: unsupported protocol: {}:",
            parsed.scheme()
        ));
    }

    let url_hash = {
        let mut h = sha2::Sha256::new();
        h.update(parsed.as_str().as_bytes());
        hex::encode(h.finalize())
    };

    let dir: PathBuf = ctx.map_or_else(|| cache_dir().clone(), |c| c.cache_dir.join("fetch"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("fetch_url: failed to create cache dir: {e}"))?;
    let cache_file = dir.join(format!("{url_hash}.txt"));

    // Serve from cache when available; download otherwise.
    let char_count = if cache_file.exists() {
        let existing = tokio::fs::read_to_string(&cache_file)
            .await
            .unwrap_or_default();
        existing.chars().count()
    } else {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_S))
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| format!("fetch_url: failed to build HTTP client: {e}"))?;

        let res = client
            .get(parsed.as_str())
            .header(
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120 Safari/537.36",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .send()
            .await
            .map_err(|e| format!("fetch_url: request failed: {e}"))?;

        if !res.status().is_success() {
            return Err(format!("fetch_url: HTTP {}", res.status()));
        }

        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();

        let body = res
            .text()
            .await
            .map_err(|e| format!("fetch_url: failed to read body: {e}"))?;

        let text =
            if content_type.contains("text/html") || content_type.contains("application/xhtml") {
                html_to_text(&body)
            } else {
                body
            };

        let cc = text.chars().count();
        tokio::fs::write(&cache_file, &text)
            .await
            .map_err(|e| format!("fetch_url: failed to write cache: {e}"))?;
        cc
    };

    // Run postprocess.
    let cache_str = cache_file.to_string_lossy().into_owned();
    let bash_cmd = format!("{postprocess} < '{cache_str}'");
    let out = run_subprocess("bash", &["-c".into(), bash_cmd]).await?;

    // Treat exit codes 0 and 1 as success (grep, diff, and friends use 1 for
    // "no match" / "differs" — not a real error). Anything else, plus a
    // signal kill (`code = None`), is surfaced as a postprocess error.
    let pp_is_error = !matches!(out.code, Some(0 | 1));

    // Build result string using `write!` (infallible for String) to avoid
    // the `format_push_string` lint.
    let mut result = format!("Cached: {cache_str} ({char_count} chars)\n");
    let _ = write!(result, "\n--- postprocess: {postprocess} ---\n");

    if pp_is_error {
        let err_text = if out.stderr.trim().is_empty() {
            match out.code {
                Some(c) => format!("[exit code {c}]"),
                None => "[killed by signal]".to_owned(),
            }
        } else {
            out.stderr.trim().to_owned()
        };
        let _ = write!(result, "[error] {err_text}");
    } else if out.stdout.trim().is_empty() {
        result.push_str("(no output)");
    } else {
        let pp_log_path = make_fetch_pp_log_path(ctx);
        let capped = cap_and_tee(
            out.stdout.as_bytes(),
            PP_CAP,
            TruncationBias::Head,
            &pp_log_path,
        )
        .await
        .map_err(|e| format!("fetch_url: failed to write postprocess tee log: {e}"))?;
        result.push_str(&capped.body);
    }

    result.push_str("\n--- end ---");

    Ok(result)
}

// ---------------------------------------------------------------------------
// Shell-gated execution (no shell-tool in selection)
// ---------------------------------------------------------------------------

/// Execute `fetch_url` when no shell-execution tool is in the selection.
///
/// No shell pipeline is spawned.  The cached plain text is read and
/// truncated to at most [`SHELL_GATED_MAX_LINES`] lines or
/// [`SHELL_GATED_MAX_BYTES`] bytes, whichever limit is reached first.
/// A truncation marker is appended when the cap is hit:
/// `\n... [content truncated: N lines / M chars suppressed]`.
async fn execute_shell_gated(url_str: String, ctx: Option<&ToolCtx>) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(&url_str).map_err(|_| format!("fetch_url: invalid URL: {url_str}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "fetch_url: unsupported protocol: {}:",
            parsed.scheme()
        ));
    }

    let url_hash = {
        let mut h = sha2::Sha256::new();
        h.update(parsed.as_str().as_bytes());
        hex::encode(h.finalize())
    };

    let dir: PathBuf = ctx.map_or_else(|| cache_dir().clone(), |c| c.cache_dir.join("fetch"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("fetch_url: failed to create cache dir: {e}"))?;
    let cache_file = dir.join(format!("{url_hash}.txt"));

    // Serve from cache when available; download otherwise.
    let text = if cache_file.exists() {
        tokio::fs::read_to_string(&cache_file)
            .await
            .unwrap_or_default()
    } else {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_S))
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| format!("fetch_url: failed to build HTTP client: {e}"))?;

        let res = client
            .get(parsed.as_str())
            .header(
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120 Safari/537.36",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .send()
            .await
            .map_err(|e| format!("fetch_url: request failed: {e}"))?;

        if !res.status().is_success() {
            return Err(format!("fetch_url: HTTP {}", res.status()));
        }

        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();

        let body = res
            .text()
            .await
            .map_err(|e| format!("fetch_url: failed to read body: {e}"))?;

        let t = if content_type.contains("text/html") || content_type.contains("application/xhtml")
        {
            html_to_text(&body)
        } else {
            body
        };

        tokio::fs::write(&cache_file, &t)
            .await
            .map_err(|e| format!("fetch_url: failed to write cache: {e}"))?;
        t
    };

    let cache_str = cache_file.to_string_lossy().into_owned();
    let total_chars = text.chars().count();

    // Apply line and byte caps.
    let (body, suppressed_lines, suppressed_chars) =
        apply_shell_gated_cap(&text, SHELL_GATED_MAX_LINES, SHELL_GATED_MAX_BYTES);

    Ok(format_shell_gated_result(
        &cache_str,
        total_chars,
        &body,
        suppressed_lines,
        suppressed_chars,
    ))
}

/// Format the final output string for shell-gated `fetch_url`.
///
/// Extracted as a pure function so that the `suppressed_lines > 0 || suppressed_chars > 0`
/// condition can be unit-tested without network access.
fn format_shell_gated_result(
    cache_str: &str,
    total_chars: usize,
    body: &str,
    suppressed_lines: usize,
    suppressed_chars: usize,
) -> String {
    let mut result = format!("Cached: {cache_str} ({total_chars} chars)\n\n");
    result.push_str(body);
    if suppressed_lines > 0 || suppressed_chars > 0 {
        let _ = write!(
            result,
            "\n... [content truncated: {suppressed_lines} lines / {suppressed_chars} chars suppressed]"
        );
    }
    result
}

/// Truncate `text` to at most `max_lines` lines or `max_bytes` bytes,
/// whichever limit is reached first.  Returns `(body, suppressed_lines,
/// suppressed_chars)`.  When neither limit is exceeded, `suppressed_*` are
/// both zero and `body` equals `text`.
fn apply_shell_gated_cap(text: &str, max_lines: usize, max_bytes: usize) -> (String, usize, usize) {
    let mut byte_count = 0usize;
    let mut kept_end = 0usize; // byte offset into `text` of the last kept character
    let mut capped = false;

    for (line_idx, line) in text.lines().enumerate() {
        let line_bytes = line.len() + 1; // +1 for the newline
        if line_idx >= max_lines || byte_count + line_bytes > max_bytes {
            capped = true;
            break;
        }
        byte_count += line_bytes;
        kept_end += line_bytes;
    }

    if !capped {
        return (text.to_owned(), 0, 0);
    }

    // Ensure the slice boundary is on a char boundary.
    let safe_end = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= kept_end)
        .last()
        .unwrap_or(0);

    let body = text[..safe_end].to_owned();
    let remainder = &text[safe_end..];
    let suppressed_lines = remainder.lines().count();
    let suppressed_chars = remainder.chars().count();
    (body, suppressed_lines, suppressed_chars)
}

// ---------------------------------------------------------------------------
// Log-path helpers
// ---------------------------------------------------------------------------

/// Path for the postprocess-output tee log.
///
/// With a session context: `<ctx.cache_dir>/fetch/<ts-ms>-<tool_call_id>-pp.log`.
/// Without context (test fallback): a per-process temp directory.
fn make_fetch_pp_log_path(ctx: Option<&ToolCtx>) -> PathBuf {
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S");
    let ms = now.timestamp_subsec_millis();

    if let Some(c) = ctx {
        let filename = format!("{ts}-{ms:03}-{}-pp.log", c.tool_call_id);
        c.cache_dir.join("fetch").join(filename)
    } else {
        let filename = format!("{ts}-{ms:03}-pp.log");
        std::env::temp_dir()
            .join(format!("omega-fetch-{}", std::process::id()))
            .join(filename)
    }
}

// ---------------------------------------------------------------------------
// HTML → plain text
// ---------------------------------------------------------------------------

#[allow(clippy::expect_used)] // hardcoded patterns are always valid regex
fn html_to_text(html: &str) -> String {
    use std::borrow::Cow;

    let re_script = regex::Regex::new(r"(?si)<script[\s\S]*?</script>").expect("script regex");
    let re_style = regex::Regex::new(r"(?si)<style[\s\S]*?</style>").expect("style regex");
    let re_block = regex::Regex::new(r"(?i)</?(?:p|div|br|li|h[1-6]|tr|td|th|blockquote)[^>]*>")
        .expect("block regex");
    let re_tags = regex::Regex::new(r"<[^>]+>").expect("tags regex");
    let re_spaces = regex::Regex::new(r"[ \t]+").expect("spaces regex");
    let re_newlines = regex::Regex::new(r"\n{3,}").expect("newlines regex");

    let s: Cow<str> = re_script.replace_all(html, " ");
    let s: Cow<str> = re_style.replace_all(s.as_ref(), " ");
    let s: Cow<str> = re_block.replace_all(s.as_ref(), "\n");
    let s: Cow<str> = re_tags.replace_all(s.as_ref(), "");

    let s = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    let s: Cow<str> = re_spaces.replace_all(&s, " ");
    let s: Cow<str> = re_newlines.replace_all(s.as_ref(), "\n\n");

    s.trim().to_owned()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // apply_shell_gated_cap
    // ------------------------------------------------------------------

    /// No truncation when content fits within both limits.
    #[test]
    fn cap_no_truncation_needed() {
        let text = "hello\nworld\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 100, 10_000);
        assert_eq!(body, text);
        assert_eq!(sup_lines, 0);
        assert_eq!(sup_chars, 0);
    }

    /// Line limit triggers truncation even when byte limit is not hit.
    #[test]
    fn cap_line_limit_alone_triggers_truncation() {
        let text = "line1\nline2\nline3\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 2, 10_000);
        assert_eq!(body, "line1\nline2\n");
        assert_eq!(sup_lines, 1);
        assert_eq!(sup_chars, 6); // "line3\n" = 6 chars
    }

    /// Byte limit triggers truncation even when line limit is not hit.
    #[test]
    fn cap_byte_limit_alone_triggers_truncation() {
        // "line1\n" = 6 bytes, limit 8 → fits; "line2\n" = 6 bytes, 6+6 > 8 → capped.
        let text = "line1\nline2\nline3\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 1_000, 8);
        assert_eq!(body, "line1\n");
        assert_eq!(sup_lines, 2);
        assert_eq!(sup_chars, 12); // "line2\nline3\n" = 12 chars
    }

    /// A line that exactly fills `max_bytes` must be KEPT (`>`, not `>=`).
    #[test]
    fn cap_exact_byte_boundary_line_is_kept() {
        // "abcde\n" = 6 bytes.  With `>`, 0+6 > 6 = false → kept.
        // With `>=` mutation: 0+6 >= 6 = true → capped (wrong).
        let text = "abcde\nnext\n";
        let (body, sup_lines, _) = apply_shell_gated_cap(text, 1_000, 6);
        assert_eq!(
            body, "abcde\n",
            "line filling the byte limit exactly must be kept"
        );
        assert_eq!(sup_lines, 1);
    }

    /// Verifies the byte check uses ADDITION, not multiplication.
    ///
    /// With `byte_count + line_bytes > max_bytes` (correct): the first `"\n"` fits
    /// exactly (0+1 = 1, not > 1) and the second is capped (1+1 = 2 > 1).
    /// With the `+ → *` mutation: `0*1 = 0` and `1*1 = 1`, neither > 1, so
    /// BOTH lines would be kept (wrong).
    #[test]
    fn cap_byte_check_uses_addition_not_multiplication() {
        let text = "\n\n";
        let (body, _, sup_chars) = apply_shell_gated_cap(text, 1_000, 1);
        assert_eq!(body, "\n", "first empty line must be kept; second capped");
        assert!(sup_chars > 0, "second line must be suppressed");
    }

    /// Exactly `max_lines` lines — no truncation (tests `>=` boundary).
    #[test]
    fn cap_exact_line_count_at_limit_no_truncation() {
        let text = "a\nb\nc\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 3, 10_000);
        assert_eq!(body, text);
        assert_eq!(sup_lines, 0);
        assert_eq!(sup_chars, 0);
    }

    /// One line over `max_lines` — truncates (confirms `>=` not `>`).
    #[test]
    fn cap_one_line_over_limit_truncates() {
        let text = "a\nb\nc\nd\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 3, 10_000);
        assert_eq!(body, "a\nb\nc\n");
        assert_eq!(sup_lines, 1);
        assert_eq!(sup_chars, 2); // "d\n" = 2 chars
    }

    /// Trailing double-newline: the second `\n` becomes a lone `\n` remainder.
    ///
    /// `"a\n\n"` capped at 1 line: body=`"a\n"`, remainder=`"\n"`.  In Rust,
    /// `"\n".lines()` yields `[""]` (one empty line), so both suppressed counts
    /// equal 1.
    #[test]
    fn cap_trailing_double_newline_has_correct_suppressed_counts() {
        let text = "a\n\n";
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 1, 10_000);
        assert_eq!(body, "a\n");
        // remainder = "\n": lines().count() = 1 (empty-string line), chars().count() = 1
        assert_eq!(sup_lines, 1);
        assert_eq!(sup_chars, 1);
    }

    /// Suppressed line and char counts are correct.
    #[test]
    fn cap_suppressed_counts_are_correct() {
        let text = "one\ntwo\nthree\nfour\n";
        // max_lines=2 → body="one\ntwo\n", remainder="three\nfour\n"
        let (body, sup_lines, sup_chars) = apply_shell_gated_cap(text, 2, 10_000);
        assert_eq!(body, "one\ntwo\n");
        assert_eq!(sup_lines, 2);
        assert_eq!(sup_chars, 11); // "three\nfour\n" = 11 chars
    }

    /// `SHELL_GATED_MAX_BYTES` (50×1024 = 51200): sub-50-KB content must not be
    /// truncated.  If the constant were mutated to `50 + 1024 = 1074`, the
    /// 5000-byte content used here WOULD be truncated → test fails → caught.
    #[test]
    fn max_bytes_constant_allows_sub_50kb_content() {
        // 500 lines × 10 bytes/line = 5000 bytes — well below 50 KB
        let text = "xxxxxxxxx\n".repeat(500);
        assert_eq!(text.len(), 5000);
        let (body, sup_lines, sup_chars) =
            apply_shell_gated_cap(&text, SHELL_GATED_MAX_LINES, SHELL_GATED_MAX_BYTES);
        assert_eq!(
            sup_lines, 0,
            "5 KB must not be truncated at the 50 KB limit"
        );
        assert_eq!(sup_chars, 0);
        assert_eq!(body, text);
    }

    /// Content > 50 KB must be truncated.
    #[test]
    fn max_bytes_constant_truncates_over_50kb_content() {
        // 1000 lines × 64 bytes/line = 64 000 bytes — over 50 KB
        let chunk = "x".repeat(63); // 63 chars + 1 newline = 64 bytes
        let text = format!("{chunk}\n").repeat(1000);
        assert_eq!(text.len(), 64_000);
        let (_, sup_lines, sup_chars) =
            apply_shell_gated_cap(&text, SHELL_GATED_MAX_LINES, SHELL_GATED_MAX_BYTES);
        assert!(
            sup_chars > 0,
            "64 KB content must be truncated at the 50 KB limit"
        );
        assert!(sup_lines > 0);
    }

    // ------------------------------------------------------------------
    // format_shell_gated_result
    // ------------------------------------------------------------------

    /// No suppressed content → no truncation marker.
    #[test]
    fn format_result_no_marker_when_not_truncated() {
        let result = format_shell_gated_result("/tmp/foo.txt", 10, "body text", 0, 0);
        assert!(
            !result.contains("[content truncated"),
            "no marker expected: {result}"
        );
        assert!(result.contains("body text"));
    }

    /// Suppressed lines only → truncation marker appears (tests `||` not `&&`).
    #[test]
    fn format_result_marker_when_only_sup_lines_nonzero() {
        let result = format_shell_gated_result("/tmp/foo.txt", 20, "body", 3, 0);
        assert!(
            result.contains("[content truncated"),
            "marker expected when sup_lines=3: {result}"
        );
    }

    /// Suppressed chars only → truncation marker appears (tests `||` not `&&`).
    /// This case arises when the remainder is a bare trailing `\n` (zero logical
    /// lines in Rust's iterator, one char).
    #[test]
    fn format_result_marker_when_only_sup_chars_nonzero() {
        let result = format_shell_gated_result("/tmp/foo.txt", 100, "body", 0, 1);
        assert!(
            result.contains("[content truncated"),
            "marker expected when sup_chars=1: {result}"
        );
    }

    /// Marker includes both suppressed line and char counts.
    #[test]
    fn format_result_marker_includes_counts() {
        let result = format_shell_gated_result("/tmp/foo.txt", 100, "body", 5, 42);
        assert!(
            result.contains("5 lines"),
            "must include line count: {result}"
        );
        assert!(
            result.contains("42 chars"),
            "must include char count: {result}"
        );
    }

    /// The header line includes the cache path and total char count.
    #[test]
    fn format_result_header_includes_cache_path_and_total_chars() {
        let result = format_shell_gated_result("/my/cache/file.txt", 777, "body", 0, 0);
        assert!(
            result.contains("Cached: /my/cache/file.txt (777 chars)"),
            "header missing: {result}"
        );
    }
}
