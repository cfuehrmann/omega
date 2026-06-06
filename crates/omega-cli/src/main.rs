//! Omega command-line entry point.
//!
//! Phase 1d.0b: full agent-loop wiring.
//!
//! Usage:
//! ```text
//! ANTHROPIC_API_KEY=sk-... omega run --instruction "List files in cwd"
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use futures::StreamExt as _;
use omega_agent::{Agent, AgentConfig};
use omega_core::{AnthropicProvider, RetryConfig, RetryingProvider};
use omega_store::{ContextStore, EventStore, SESSIONS_ROOT, make_session_dir};
use omega_types::OmegaEvent;
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(
    name = "omega",
    version,
    about = "Omega software engineering agent (Rust)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a single agent turn from the command line.
    Run {
        /// User instruction to send to the agent.
        #[arg(long, allow_hyphen_values = true)]
        instruction: String,

        /// Anthropic model identifier.
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,

        /// Adaptive-thinking effort level (low / medium / high; also
        /// `xhigh` on Opus 4.7/4.8 and `max` on Opus models). Forwarded as
        /// `output_config.effort` on every Anthropic request and
        /// capped per model by `cap_effort_for_model`.
        #[arg(long, default_value = "medium")]
        effort: String,

        /// Override session root directory (default: `<cwd>/.omega/sessions`).
        #[arg(long)]
        session_root: Option<String>,

        /// Allow running when there are uncommitted git changes in the
        /// working tree.  Without this flag the command exits with an
        /// error if `git status --porcelain` reports any pending changes.
        #[arg(long)]
        allow_dirty: bool,
        /// Omit output-format rendering guidance and the interactive-discussion
        /// policy from the core prompt.  Use for headless / benchmark runs.
        #[arg(long)]
        headless: bool,

        /// Tool-selection preset.  Single source of truth for the CLI and the
        /// (forthcoming) UI is `omega_tools::PRESETS`.
        ///
        /// Presets:
        ///   - `standard`     — 12 tools — file ops, shell, web (no REPL)
        ///   - `all`          — all 13 — standard plus `python_repl`
        ///   - `repl-centric` — `python_repl` + `web_search` + `fetch_url`
        ///
        /// Omitting the flag is equivalent to `--preset standard` at the
        /// agent level (server falls back to `DEFAULT_TOOL_NAMES`).
        #[arg(long, value_parser = parse_preset)]
        preset: Option<&'static omega_tools::Preset>,
    },
}

/// Resolve `--preset <id>` against [`omega_tools::PRESETS`].
///
/// Returns a clap-friendly error listing every known preset id when the
/// caller passes an unknown one — matches clap's own "possible values" hint
/// style.
fn parse_preset(s: &str) -> Result<&'static omega_tools::Preset, String> {
    omega_tools::preset_by_id(s).ok_or_else(|| {
        let known: Vec<&str> = omega_tools::PRESETS.iter().map(|p| p.id).collect();
        format!(
            "unknown preset '{s}'; expected one of: {}",
            known.join(", ")
        )
    })
}

#[tokio::main]
async fn main() {
    // Load .env files in priority order (first writer wins for each key):
    //   1. CWD .env  — project-level overrides
    //   2. ~/.config/omega/.env — user-level secrets (API keys, etc.)
    //   3. Real environment variables — highest priority, never overridden
    dotenvy::dotenv().ok();
    if let Ok(home) = std::env::var("HOME") {
        dotenvy::from_path(std::path::Path::new(&home).join(".config/omega/.env")).ok();
    }

    let cli = Cli::parse();
    let exit_code = match cli.command {
        Command::Run {
            instruction,
            model,
            effort,
            session_root,
            allow_dirty,
            headless,
            preset,
        } => {
            run(
                instruction,
                model,
                effort,
                session_root,
                allow_dirty,
                headless,
                preset,
            )
            .await
        }
    };
    std::process::exit(exit_code);
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run(
    instruction: String,
    model: String,
    effort: String,
    session_root: Option<String>,
    allow_dirty: bool,
    headless: bool,
    preset: Option<&'static omega_tools::Preset>,
) -> i32 {
    // ---- API key -------------------------------------------------------
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("omega: ANTHROPIC_API_KEY is not set");
            return 1;
        }
    };

    // ---- CWD -----------------------------------------------------------
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("omega: cannot determine cwd: {e}");
            return 1;
        }
    };

    // ---- Pending-changes gate ------------------------------------------
    // Refuse to run if the working tree has uncommitted changes, unless the
    // caller explicitly opted in with --allow-dirty.
    if !allow_dirty && git_has_pending_changes(&cwd) {
        eprintln!(
            "omega: there are uncommitted changes in the working tree.\n\
             Commit or stash them before running omega, or pass --allow-dirty \
             to proceed anyway."
        );
        return 1;
    }

    // ---- Session directory ---------------------------------------------
    let root = session_root
        .as_deref()
        .map_or_else(|| cwd.join(SESSIONS_ROOT), PathBuf::from);

    let paths = match make_session_dir(&root).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("omega: failed to create session dir: {e}");
            return 1;
        }
    };

    eprintln!("Session: {}", paths.dir.display());

    // ---- Stores --------------------------------------------------------
    let event_store = EventStore::new(paths.events_file.clone());
    let context_store = ContextStore::new(paths.context_file.clone());

    // ---- Provider ------------------------------------------------------
    // ANTHROPIC_BASE_URL: documented Anthropic-SDK env var. Used by
    // tests to point at a local axum SSE fake, and by users to route
    // through corporate proxies.
    let anthropic = if let Ok(u) = std::env::var("ANTHROPIC_BASE_URL") {
        AnthropicProvider::new(api_key).with_base_url(u)
    } else {
        AnthropicProvider::new(api_key)
    }
    // BUG-D: context-management betas required for `clear_tool_uses_20250919`,
    // `clear_thinking_20251015`, and `compact_20260112` edit types.
    .with_beta("compact-2026-01-12")
    .with_beta("context-management-2025-06-27");
    // OMEGA_RETRY_INITIAL_MS: test-only knob for the initial retry
    // backoff. Production uses the default (500 ms) — keeping retry
    // tests bounded to single-digit milliseconds.
    let initial_backoff = std::env::var("OMEGA_RETRY_INITIAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(
            RetryConfig::default().initial_backoff,
            Duration::from_millis,
        );
    let provider = Arc::new(RetryingProvider::new(
        anthropic,
        RetryConfig {
            max_attempts: 4,
            initial_backoff,
            ..RetryConfig::default()
        },
    ));

    // ---- Agent ---------------------------------------------------------
    //
    // `agent.init()` discovers `AGENTS.md` (global + repo tiers),
    // assembles the system blocks, and writes the `server_started` +
    // `session_started` events.  Single code path shared with the
    // server — see `omega-server/src/router.rs`.
    let config = AgentConfig {
        model: model.clone(),
        effort: Some(effort.clone()),
        cwd: cwd.clone(),
        session_dir: paths.dir.clone(),
        headless,
        features: None, // resolved from env in agent.init()
        // None (no `--preset` flag) lets `Agent::new` apply
        // `omega_tools::DEFAULT_TOOL_NAMES` — functionally equal to passing
        // `--preset standard`, but distinguishable on the event log: an
        // explicit preset materialises as `Some(…)` in the
        // `session_started` event for forensics.
        tool_selection: preset.map(|p| p.tools.iter().map(|s| (*s).to_owned()).collect()),
    };
    let mut agent = Agent::new(provider, context_store, event_store, config);
    if let Err(e) = agent.init().await {
        eprintln!("omega: failed to initialise session: {e}");
        return 1;
    }

    // ---- Ctrlc cancel --------------------------------------------------
    let cancel = CancellationToken::new();
    let cancel_ctrlc = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_ctrlc.cancel();
        }
    });

    // ---- Stream the turn -----------------------------------------------
    // Headless one-shot: feed the single instruction through the persistent
    // run loop's inbox, then close the inbox so the loop terminates after
    // this one turn instead of parking.
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::channel(1);
    let _ = inbox_tx
        .send(omega_agent::InputItem::Human {
            content: instruction,
        })
        .await;
    drop(inbox_tx);
    let mut stream = agent.run(inbox_rx, cancel);

    let mut exit_code = 0i32;

    while let Some(item) = stream.next().await {
        match item {
            omega_core::AgentItem::Signal(sig) => match sig {
                omega_types::StreamSignal::Text { text, .. } => {
                    print!("{text}");
                }
                omega_types::StreamSignal::Thinking { .. }
                | omega_types::StreamSignal::ThinkingBlockComplete { .. }
                | omega_types::StreamSignal::TextBlockComplete { .. }
                | omega_types::StreamSignal::ToolUseBlockStart { .. }
                | omega_types::StreamSignal::ToolInput { .. }
                | omega_types::StreamSignal::ToolUseBlockComplete { .. } => {
                    // Thinking, tool-use streaming signals and block
                    // completion markers are not shown in CLI output.
                }
            },
            omega_core::AgentItem::Event(boxed) => {
                let ev = *boxed;
                match &ev {
                    OmegaEvent::TurnEnd(te) => {
                        println!();
                        eprintln!(
                            "\n[turn complete | in={} out={} cache_hit={} cache_write={}]",
                            te.metrics.input_tokens,
                            te.metrics.output_tokens,
                            te.metrics.cache_read_tokens.unwrap_or(0),
                            te.metrics.cache_creation_tokens.unwrap_or(0),
                        );
                        exit_code = 0;
                    }
                    OmegaEvent::TurnInterrupted(ti) => {
                        println!();
                        eprintln!(
                            "\n[turn interrupted: {}]",
                            ti.reason
                                .as_ref()
                                .map_or_else(|| "unknown".to_owned(), |r| format!("{r:?}"))
                        );
                        exit_code = 1;
                    }
                    OmegaEvent::AgentError(ae) => {
                        eprintln!("\n[agent error: {}]", ae.error);
                    }
                    OmegaEvent::ToolCall(tc) => {
                        eprintln!("\n[tool: {}]", tc.name);
                    }
                    OmegaEvent::ToolResult(tr) => {
                        let preview: String = tr.output.chars().take(120).collect();
                        eprintln!(
                            "[result{}: {}…]",
                            if tr.is_error { " (error)" } else { "" },
                            preview
                        );
                    }
                    OmegaEvent::LlmCall(_) => {
                        eprint!(".");
                    }
                    _ => {}
                }
            }
        }
    }

    // Phase 4: drop the stream (releases the mutable borrow on `agent`),
    // then kill any still-running monitors and persist
    // MonitorStopped(StoppedBySessionEnd) for each.  The stream has already
    // drained, so the agent loop is done writing — single-writer preserved.
    drop(stream);
    agent.shutdown_and_log_monitors().await;

    exit_code
}

/// Returns `true` if `git status --porcelain` reports any uncommitted changes
/// in `cwd`.  Returns `false` if the tree is clean, git is absent, or `cwd`
/// is not a repository (fail-open).
///
/// When `OMEGA_ALLOW_DIRTY` is set, the check is skipped and `false` is
/// returned — used by test harnesses running against a dirty working tree.
fn git_has_pending_changes(cwd: &std::path::Path) -> bool {
    if std::env::var("OMEGA_ALLOW_DIRTY").is_ok() {
        return false;
    }
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .is_ok_and(|o| !o.stdout.is_empty())
}
