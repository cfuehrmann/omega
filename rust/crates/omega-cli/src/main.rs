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
use omega_agent::{
    Agent, AgentConfig, max_output_tokens_for_model, system_prompt::read_system_prompt_append,
    system_prompt::system_prompt_append_path,
};
use omega_core::{AnthropicProvider, RetryConfig, RetryingProvider};
use omega_protocol::{OmegaEvent, events::SessionStartedEvent};
use omega_store::{ContextStore, EventStore, SESSIONS_ROOT, make_session_dir};
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(name = "omega", about = "Omega coding agent (Rust)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a single agent turn from the command line.
    Run {
        /// User instruction to send to the agent.
        #[arg(long)]
        instruction: String,

        /// Anthropic model identifier.
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,

        /// Reasoning effort level (low / medium / high). Recorded but
        /// not yet wired into the thinking budget in Phase 1d.0b.
        #[arg(long, default_value = "medium")]
        effort: String,

        /// Override session root directory (default: `<cwd>/.omega/sessions`).
        #[arg(long)]
        session_root: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let exit_code = match cli.command {
        Command::Run {
            instruction,
            model,
            effort,
            session_root,
        } => run(instruction, model, effort, session_root).await,
    };
    std::process::exit(exit_code);
}

#[allow(clippy::too_many_lines)]
async fn run(
    instruction: String,
    model: String,
    effort: String,
    session_root: Option<String>,
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

    // ---- System prompt -------------------------------------------------
    let spa_path = system_prompt_append_path(&cwd);
    let system_prompt_append = read_system_prompt_append(&spa_path).await.unwrap_or(None);
    let max_tokens = max_output_tokens_for_model(&model);
    let system_prompt = omega_agent::build_system_prompt(
        &cwd.to_string_lossy(),
        max_tokens,
        system_prompt_append.as_deref(),
    );

    // ---- session_started event -----------------------------------------
    let session_id = paths.dir.file_name().map_or_else(
        || "unknown".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );

    let session_path = paths
        .dir
        .strip_prefix(&cwd)
        .unwrap_or(&paths.dir)
        .to_string_lossy()
        .into_owned();

    let session_started = OmegaEvent::SessionStarted(SessionStartedEvent {
        time: now_iso(),
        session_id,
        path: session_path,
        model: model.clone(),
        effort: effort.clone(),
        system_prompt,
    });
    if let Err(e) = event_store.append(&session_started).await {
        eprintln!("omega: failed to write session_started: {e}");
        return 1;
    }

    // ---- Provider ------------------------------------------------------
    // ANTHROPIC_BASE_URL: documented Anthropic-SDK env var. Used by
    // tests to point at a local axum SSE fake, and by users to route
    // through corporate proxies.
    let anthropic = if let Ok(u) = std::env::var("ANTHROPIC_BASE_URL") {
        AnthropicProvider::new(api_key).with_base_url(u)
    } else {
        AnthropicProvider::new(api_key)
    };
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
    let config = AgentConfig {
        model,
        effort: None,
        cwd: cwd.clone(),
        system_prompt_append,
        session_dir: paths.dir.clone(),
    };
    let mut agent = Agent::new(provider, context_store, event_store, config);

    // ---- Ctrlc cancel --------------------------------------------------
    let cancel = CancellationToken::new();
    let cancel_ctrlc = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_ctrlc.cancel();
        }
    });

    // ---- Stream the turn -----------------------------------------------
    let mut stream = agent.send_message(instruction, cancel);

    let mut exit_code = 0i32;

    while let Some(item) = stream.next().await {
        match item {
            omega_core::AgentItem::Signal(sig) => match sig {
                omega_protocol::StreamSignal::Text { text } => {
                    print!("{text}");
                }
                omega_protocol::StreamSignal::Thinking { .. }
                | omega_protocol::StreamSignal::ThinkingBlockComplete { .. } => {
                    // Thinking blocks and their completion signals are not
                    // shown in CLI output.
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

    exit_code
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
