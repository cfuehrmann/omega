"""
Harbor agent adapter for Omega.

Usage
-----
harbor run -d terminal-bench@2.0 \\
  --agent-import-path omega_agent:OmegaAgent \\
  -m anthropic/claude-sonnet-4-6 \\
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY

Optional kwargs (via --agent-kwargs or subclass override):
  max_turns  int    LLM-call budget per task (default 50)
  effort     str    Thinking effort: low|medium|high|max|xhigh (default medium)
"""

from __future__ import annotations

import json
import shlex
from typing import TYPE_CHECKING

from harbor.agents.installed.base import BaseInstalledAgent, CliFlag, with_prompt_template

if TYPE_CHECKING:
    from harbor.environments.base import BaseEnvironment
    from harbor.models.agent.context import AgentContext

OMEGA_VERSION = "v0.1.0"
OMEGA_REPO = "https://github.com/cfuehrmann/omega"
OMEGA_SESSION_DIR = "/tmp/omega-session"
OMEGA_INSTALL_DIR = "/home/agent/omega"
RUN_TIMEOUT_SEC = 600  # 10 minutes per task


class OmegaAgent(BaseInstalledAgent):
    """Omega installed into the task container and invoked via src/cli.ts."""

    CLI_FLAGS = [
        CliFlag(kwarg="max_turns", cli="--max-turns", type="int", default=50),
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
            command="curl -fsSL https://bun.sh/install | bash",
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

        flags = self.build_cli_flags()
        cmd = (
            f"cd {OMEGA_INSTALL_DIR}"
            f" && ~/.bun/bin/bun run src/cli.ts run"
            f" --instruction {shlex.quote(instruction)}"
            f" --model {shlex.quote(self._parsed_model_name)}"
            f" --session-dir {OMEGA_SESSION_DIR}"
            f" {flags}"
        )

        try:
            await self.exec_as_agent(
                environment, command=cmd, timeout_sec=RUN_TIMEOUT_SEC
            )
        finally:
            # Always pull events.jsonl to the host logs dir so that
            # populate_context_post_run and Harbor's UI can read it,
            # even when the CLI exits non-zero.
            try:
                await environment.download_file(
                    f"{OMEGA_SESSION_DIR}/events.jsonl",
                    self.logs_dir / "events.jsonl",
                )
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
