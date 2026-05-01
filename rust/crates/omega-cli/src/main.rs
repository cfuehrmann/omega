//! Omega command-line entry point.
//!
//! Phase 1d.0a scaffold: parses `--help` and `run --instruction ...` but
//! does not yet drive the agent. Wired up in 1d.0b.

use clap::{Parser, Subcommand};

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
        /// not yet wired into the request in Phase 1d.0a.
        #[arg(long, default_value = "medium")]
        effort: String,

        /// Override session directory (default: `.omega/sessions/<auto>`).
        #[arg(long)]
        session_dir: Option<String>,

        /// Hard cap on agentic loop iterations. None = no cap.
        #[arg(long)]
        max_turns: Option<u32>,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            instruction,
            model,
            effort,
            session_dir,
            max_turns,
        } => {
            eprintln!(
                "omega run (stub): instruction={instruction:?} model={model} effort={effort} \
                 session_dir={session_dir:?} max_turns={max_turns:?}"
            );
            eprintln!("(Phase 1d.0a scaffold: agent loop wiring lands in 1d.0b)");
        }
    }
}
