//! `fetch_url` — download a URL to a content-addressed cache file, then run
//! a postprocess shell command on it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;

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

const POSTPROCESS_MAX_CHARS: usize = 8_000;
const FETCH_TIMEOUT_S: u64 = 15;

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

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let url_str = input["url"]
        .as_str()
        .ok_or("fetch_url: url is required")?
        .trim()
        .to_owned();
    let postprocess = input["postprocess"]
        .as_str()
        .ok_or("fetch_url: postprocess is required")?
        .trim()
        .to_owned();

    if url_str.is_empty() {
        return Err("fetch_url: url must not be empty".into());
    }
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

    let dir = cache_dir();
    tokio::fs::create_dir_all(dir)
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

    let mut pp_text = if pp_is_error {
        if out.stderr.trim().is_empty() {
            match out.code {
                Some(c) => format!("[exit code {c}]"),
                None => "[killed by signal]".to_owned(),
            }
        } else {
            out.stderr.trim().to_owned()
        }
    } else {
        out.stdout.clone()
    };

    let mut truncated = false;
    if pp_text.chars().count() > POSTPROCESS_MAX_CHARS {
        let end = pp_text
            .char_indices()
            .nth(POSTPROCESS_MAX_CHARS)
            .map_or(pp_text.len(), |(i, _)| i);
        pp_text.truncate(end);
        truncated = true;
    }

    // Build result string using `write!` (infallible for String) to avoid
    // the `format_push_string` lint.
    let mut result = format!("Cached: {cache_str} ({char_count} chars)\n");
    let _ = write!(result, "\n--- postprocess: {postprocess} ---\n");
    if pp_is_error {
        let _ = write!(result, "[error] {pp_text}");
    } else if pp_text.trim().is_empty() {
        result.push_str("(no output)");
    } else {
        result.push_str(pp_text.trim_end());
        if truncated {
            let _ = write!(
                result,
                "\n[postprocess output truncated at {POSTPROCESS_MAX_CHARS} chars \
                 — use read_file or grep_files on the cached file for more]"
            );
        }
    }
    result.push_str("\n--- end ---");

    Ok(result)
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
