"""
Harbor agent adapter for Omega.

Usage
-----
harbor run -d terminal-bench@2.0 \\
  --agent-import-path omega_agent:OmegaAgent \\
  -m anthropic/claude-sonnet-4-6 \\
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY

Optional kwargs (via --agent-kwargs or subclass override):
  max_turns  int    LLM-call budget per task (default 100)
  effort     str    Thinking effort: low|medium|high|max|xhigh (default medium)
"""

from __future__ import annotations

import json
import shlex
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING

from harbor.agents.installed.base import BaseInstalledAgent, CliFlag, with_prompt_template

if TYPE_CHECKING:
    from harbor.environments.base import BaseEnvironment
    from harbor.models.agent.context import AgentContext

OMEGA_VERSION = "v0.1.5"
OMEGA_RUST_VERSION = "v0.1.5"  # kept in sync with OMEGA_VERSION during the migration
OMEGA_REPO = "https://github.com/cfuehrmann/omega"
OMEGA_SESSION_DIR = "/tmp/omega-session"
OMEGA_INSTALL_DIR = "/home/agent/omega"
OMEGA_RUST_INSTALL_DIR = "/home/agent/omega-rust"
OMEGA_RUST_BIN = f"{OMEGA_RUST_INSTALL_DIR}/rust/target/release/omega"
# Sessions are written into Harbor's bind-mounted agent logs directory
# (/logs/agent inside the container <-> trial_dir/agent on the host).  This
# means events.jsonl and context.jsonl appear on the host filesystem live
# during the run -- no explicit download step, and survives abrupt
# termination (timeout, OOM) where a finally block wouldn't run.  Harbor's
# `chmod 777 /logs/agent` at container start ensures the agent user can
# write here, and the recursive chown at teardown fixes host-side ownership.
OMEGA_RUST_SESSION_ROOT = "/logs/agent/omega-session"
# No Python-side timeout: harbor wraps run() with asyncio.wait_for(timeout=task.agent_timeout_sec),
# which fires an AgentTimeoutError at the correct per-task deadline.  Adding our own inner
# timeout_sec would fire earlier for long-deadline tasks (e.g. winning-avg-corewars has 3600 s)
# and raise RuntimeError instead of AgentTimeoutError, corrupting results.


class OmegaAgent(BaseInstalledAgent):
    """Omega installed into the task container and invoked via src/cli.ts."""

    CLI_FLAGS = [
        CliFlag(kwarg="max_turns", cli="--max-turns", type="int", default=100),
        CliFlag(
            kwarg="effort",
            cli="--effort",
            type="enum",
            choices=["low", "medium", "high", "max", "xhigh"],
            default="medium",
        ),
    ]

    @staticmethod
    def name() -> str:
        return "omega"

    def version(self) -> str | None:
        return OMEGA_VERSION.lstrip("v")

    # ------------------------------------------------------------------
    # Install
    # ------------------------------------------------------------------

    async def install(self, environment: "BaseEnvironment") -> None:
        await self.exec_as_root(
            environment,
            command=(
                "apt-get update -qq"
                " && apt-get install -y --no-install-recommends curl git ca-certificates unzip"
            ),
        )
        await self.exec_as_agent(
            environment,
            command="mkdir -p \"$HOME\" && touch \"$HOME/.bashrc\" && curl -fsSL https://bun.sh/install | bash",
        )
        await self.exec_as_agent(
            environment,
            command=(
                f"git clone --branch {OMEGA_VERSION} --depth 1"
                f" {OMEGA_REPO} {OMEGA_INSTALL_DIR}"
                f" && cd {OMEGA_INSTALL_DIR}"
                f" && ~/.bun/bin/bun install --frozen-lockfile"
            ),
        )

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _get_agent_timeout_sec(self) -> float | None:
        """Return the effective per-task agent timeout by reading harbor's trial config
        and the cached task.toml.  Returns None if the information is unavailable."""
        config_path = self.logs_dir.parent / "config.json"
        if not config_path.exists():
            return None
        with config_path.open() as f:
            config = json.load(f)

        # Honor any explicit per-run override first.
        override = (config.get("agent") or {}).get("override_timeout_sec")
        if override is not None:
            return float(override)

        # Locate task.toml in the harbor cache by task name.
        task_name = (config.get("task") or {}).get("path")
        if not task_name:
            return None
        task_name = Path(task_name).name  # strip any leading "terminal-bench/" prefix
        cache_root = Path.home() / ".cache" / "harbor" / "tasks"
        matches = list(cache_root.glob(f"*/{task_name}/task.toml"))
        if len(matches) != 1:
            return None
        with matches[0].open("rb") as f:
            task_config = tomllib.load(f)

        base_timeout = (task_config.get("agent") or {}).get("timeout_sec")
        if base_timeout is None:
            return None

        # Apply the same multiplier + cap logic harbor uses.
        multiplier = (
            config.get("agent_timeout_multiplier")
            or config.get("timeout_multiplier")
            or 1.0
        )
        cap = (config.get("agent") or {}).get("max_timeout_sec") or float("inf")
        return min(float(base_timeout) * float(multiplier), cap)

    # ------------------------------------------------------------------
    # Run
    # ------------------------------------------------------------------

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: "BaseEnvironment",
        context: "AgentContext",
    ) -> None:
        if self._parsed_model_provider != "anthropic":
            raise ValueError(
                f"Omega is Anthropic-only; got provider "
                f"'{self._parsed_model_provider}'. "
                f"Pass e.g. -m anthropic/claude-sonnet-4-6."
            )

        # Prepend the per-task deadline so the agent can honour the
        # half-budget rule in its core prompt ("commit a working solution
        # before refining").  Fails gracefully if timeout is unavailable.
        timeout_sec = self._get_agent_timeout_sec()
        if timeout_sec is not None:
            minutes = int(timeout_sec) // 60
            instruction = (
                f"Time budget: {int(timeout_sec)} seconds ({minutes} minutes).\n\n"
                + instruction
            )

        flags = self.build_cli_flags()
        cmd = (
            f"cd /app || true"
            f" && ~/.bun/bin/bun run {OMEGA_INSTALL_DIR}/src/cli.ts run"
            f" --instruction {shlex.quote(instruction)}"
            f" --model {shlex.quote(self._parsed_model_name)}"
            f" --session-dir {OMEGA_SESSION_DIR}"
            f" {flags}"
        )

        try:
            await self.exec_as_agent(environment, command=cmd)
        finally:
            for src, dest in (
                ("/tmp/omega-session/events.jsonl", "events.jsonl"),
                ("/tmp/omega-session/context.jsonl", "context.jsonl"),
            ):
                try:
                    await environment.download_file(src, self.logs_dir / dest)
                except Exception:
                    pass

    # ------------------------------------------------------------------
    # Context / trajectory
    # ------------------------------------------------------------------

    def populate_context_post_run(self, context: "AgentContext") -> None:
        """Read aggregate token counts from the downloaded events.jsonl."""
        events_file = self.logs_dir / "events.jsonl"
        if not events_file.exists():
            return

        with events_file.open() as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue

                if event.get("type") == "turn_end":
                    metrics = event.get("metrics", {})
                    context.n_input_tokens = metrics.get("inputTokens")
                    context.n_output_tokens = metrics.get("outputTokens")
                    cache_read = metrics.get("cacheReadTokens", 0) or 0
                    cache_write = metrics.get("cacheCreationTokens", 0) or 0
                    context.n_cache_tokens = cache_read + cache_write
                    break  # only one turn_end per session



class OmegaRustAgent(OmegaAgent):
    """Omega Rust binary, built from source in the task container.

    This class supplements `OmegaAgent` (TypeScript / Bun).  The install step
    compiles the ``omega-cli`` crate instead of running ``bun install``.  The
    run step invokes the native binary directly and writes session files into
    Harbor's bind-mounted /logs/agent tree, so events.jsonl / context.jsonl
    appear on the host filesystem live (no download step required).

    ``populate_context_post_run`` is overridden because the Rust CLI creates
    a timestamped subdirectory under --session-root, so events.jsonl lives at
    ``<logs_dir>/omega-session/<timestamp>/events.jsonl`` rather than at the
    flat path used by the TS adapter.

    Usage
    -----
    harbor run -d terminal-bench@2.0 \\
      --agent-import-path omega_agent:OmegaRustAgent \\
      -m anthropic/claude-sonnet-4-6 \\
      --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY
    """

    CLI_FLAGS = [
        # --max-turns not yet implemented in Rust CLI; omit for now
        CliFlag(
            kwarg="effort",
            cli="--effort",
            type="enum",
            choices=["low", "medium", "high"],
            default="medium",
        ),
    ]

    @staticmethod
    def name() -> str:
        return "omega-rust"

    def version(self) -> str | None:
        return OMEGA_RUST_VERSION.lstrip("v")

    # ------------------------------------------------------------------
    # Install
    # ------------------------------------------------------------------

    async def install(self, environment: "BaseEnvironment") -> None:
        # 1. System build dependencies (libssl-dev needed for reqwest/openssl-sys).
        await self.exec_as_root(
            environment,
            command=(
                "apt-get update -qq"
                " && apt-get install -y --no-install-recommends"
                " curl git ca-certificates build-essential pkg-config"
                " libssl-dev"
            ),
        )
        # 2. Rust toolchain (minimal profile — no docs, no clippy etc.).
        await self.exec_as_agent(
            environment,
            command=(
                "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs"
                " | sh -s -- -y --profile minimal"
            ),
        )
        # 3. Clone the repo at the pinned tag and compile the CLI binary.
        #    The release profile produces a single self-contained binary.
        await self.exec_as_agent(
            environment,
            command=(
                f"git clone --branch {OMEGA_RUST_VERSION} --depth 1"
                f" {OMEGA_REPO} {OMEGA_RUST_INSTALL_DIR}"
                f" && cd {OMEGA_RUST_INSTALL_DIR}/rust"
                f" && ~/.cargo/bin/cargo build -p omega-cli --release"
            ),
        )

    # ------------------------------------------------------------------
    # Run
    # ------------------------------------------------------------------

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: "BaseEnvironment",
        context: "AgentContext",
    ) -> None:
        if self._parsed_model_provider != "anthropic":
            raise ValueError(
                f"Omega is Anthropic-only; got provider "
                f"'{self._parsed_model_provider}'. "
                f"Pass e.g. -m anthropic/claude-sonnet-4-6."
            )

        timeout_sec = self._get_agent_timeout_sec()
        if timeout_sec is not None:
            minutes = int(timeout_sec) // 60
            instruction = (
                f"Time budget: {int(timeout_sec)} seconds ({minutes} minutes).\n\n"
                + instruction
            )

        flags = self.build_cli_flags()
        # Sessions go straight into the bind-mounted /logs/agent tree.
        # events.jsonl and context.jsonl are visible on the host the moment
        # the kernel flushes -- no cp dance, no download_file, and crucially
        # the data survives even if this exec is killed mid-flight (e.g.
        # AgentTimeoutError) because nothing has to run afterwards to get it
        # out of the container.
        cmd = (
            f"mkdir -p {OMEGA_RUST_SESSION_ROOT}"
            f" && cd /app || true"
            f" && {OMEGA_RUST_BIN} run"
            f" --instruction {shlex.quote(instruction)}"
            f" --model {shlex.quote(self._parsed_model_name)}"
            f" --session-root {OMEGA_RUST_SESSION_ROOT}"
            f" --headless"
            f" {flags}"
        )
        await self.exec_as_agent(environment, command=cmd)

    # ------------------------------------------------------------------
    # Context / trajectory
    # ------------------------------------------------------------------

    def populate_context_post_run(self, context: "AgentContext") -> None:
        """Read aggregate token counts from the latest session's events.jsonl.

        The Rust CLI writes to ``<logs_dir>/omega-session/<timestamp>/`` (a
        timestamped subdir created by omega's ``make_session_dir``), so we
        pick the newest one.  In a normal run there is only one.
        """
        session_root = self.logs_dir / "omega-session"
        if not session_root.is_dir():
            return
        subdirs = sorted(
            (p for p in session_root.iterdir() if p.is_dir()),
            key=lambda p: p.name,
        )
        if not subdirs:
            return
        events_file = subdirs[-1] / "events.jsonl"
        if not events_file.exists():
            return

        with events_file.open() as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue

                if event.get("type") == "turn_end":
                    metrics = event.get("metrics", {})
                    context.n_input_tokens = metrics.get("inputTokens")
                    context.n_output_tokens = metrics.get("outputTokens")
                    cache_read = metrics.get("cacheReadTokens", 0) or 0
                    cache_write = metrics.get("cacheCreationTokens", 0) or 0
                    context.n_cache_tokens = cache_read + cache_write
                    break  # only one turn_end per session
