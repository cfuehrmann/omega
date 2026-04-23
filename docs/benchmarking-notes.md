# Benchmarking Omega — research notes

Research session notes on subjecting Omega to agent benchmarks and comparing
it with other agents.

## 1. Goal

Run Omega against standard agent benchmarks so we can publish
apples-to-apples comparisons against Claude Code, Terminus-2,
Mini-SWE-Agent, OpenHands, etc. on the same models (Sonnet 4.6, Opus 4.7).

## 2. Benchmark landscape (for a terminal/coding agent)

Benchmarks that make sense for Omega. Omitted: OSWorld/WebArena (GUI),
GAIA (web browsing + multimodal), τ-bench (customer-service tool-use).

| Benchmark | What it tests | Size | Popularity | Harbor-runnable? |
|---|---|---|---|---|
| **SWE-Bench Verified** | Patch real Python GitHub issues, tests must pass | 500 | ★★★★★ (the standard) | ✅ via registry |
| **SWE-Bench Pro** | Like Verified but much harder; top agents ~23% | ~1k | ★★★ (rising) | Probably — check registry |
| **Terminal-Bench 2** | General terminal tasks (coding + sysadmin + data) | ~80 | ★★★★ (rising fast) | ✅ native |
| **SWE-Bench Multilingual** | Like Verified, non-Python | ~300 | ★★ | Likely via registry |
| **Aider polyglot** | Edit Exercism-style exercises across 6 languages | 225 | ★★★ | ❌ (own harness) |
| **AppWorld** | Multi-step tasks across app APIs via code | ~750 | ★★ | ✅ via registry |
| **LiveCodeBench** | Competitive programming, one-shot | varies | ★★★★ | Not an *agent* benchmark |

**Key insight:** because the Harbor dataset registry adapts SWE-Bench
Verified (and others) into the same harness, **one Omega integration into
Harbor gives us access to multiple benchmarks** with just a flag change.
This materially de-risks committing to Harbor.

### Recommended staging

1. ✅ **Terminal-Bench 2 oracle sweep** — done. 76/89 tasks pass (85.4%).
   13 tasks fail even for the oracle (GPU, large downloads, heavy builds) —
   these are excluded from all agent comparisons. Effective benchmark: 76 tasks.
2. ✅ **Build `src/cli.ts`** — done. Headless entrypoint with `--instruction`,
   `--model`, `--effort`, `--session-dir`, `--max-turns`. LLM text → stdout,
   structured logs → stderr. Exit 0 on `turn_end`, 1 on interrupt/error.
3. ✅ **Publish a pinned Omega version** — tagged `v0.1.0` on `develop`.
4. ✅ **Write `omega_agent.py`** — done. Lives at the repo root.
   - Installs Omega via `git clone --branch v0.1.0 --depth 1`
   - Invokes `src/cli.ts run`, downloads `events.jsonl` post-run
   - Populates token counts in `AgentContext` from `turn_end` event
   - `--agent-import-path omega_agent:OmegaAgent` (no `./` prefix)
   - `RUN_TIMEOUT_SEC = 1800` (30 min per task — increased from 600 s
     after `crack-7z-hash` hit the limit at 10 m 57 s)
5. ✅ **Install Docker + Harbor** — already installed. Repo is public.
   `carsten` is in the `docker` group; `docker ps` works without sudo.
6. 🔄 **Cost-calibration smoke test** — in progress.

   Three attempts at `crack-7z-hash` during sessions #2 and #3:

   - **Attempt 1** (`./omega_agent` import path): `TypeError` — Harbor's
     `importlib.import_module` rejects the `./` prefix. Fixed: drop `./`.
   - **Attempt 2** (missing `unzip`): Bun's installer requires `unzip`;
     not present in the task container. Fixed: added to `apt-get install`.
   - **Attempt 3** (timeout): Harbor killed the run at 600 s; the task
     took 10 m 57 s. Fixed: `RUN_TIMEOUT_SEC` raised to 1800 s.

   Additionally, two Omega-side bugs surfaced and were fixed in session #3:
   - `wait_for_output` used literal `content.includes()` instead of regex;
     patterns like `"Error|done|ready"` never matched. Now uses `RegExp`.
   - `wait_for_output` ignored the abort signal, causing the UI to hang
     when the user pressed Abort. Now checks and cancels on abort.

   **Current state: all known blockers resolved. Ready to retry.**

   Run the smoke test:
   ```bash
   cd ~/omega/dev
   docker ps    # confirm docker works without sudo
   harbor run -d terminal-bench@2.0 \
     --agent-import-path omega_agent:OmegaAgent \
     -m anthropic/claude-sonnet-4-6 \
     --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
     -t terminal-bench/crack-7z-hash \
     -n 1
   ```
   Expected outcome: `1/1 Mean: X.XXX` with X > 0, no Exception row.
   If it passes, check `jobs/<timestamp>/crack-7z-hash*/result.json`
   for the score, then expand to 5–10 tasks:
   ```bash
   harbor run -d terminal-bench@2.0 \
     --agent-import-path omega_agent:OmegaAgent \
     -m anthropic/claude-sonnet-4-6 \
     --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
     -n 1 --n-tasks 5
   ```
   Check Anthropic console for actual spend, extrapolate to 76 tasks.

   **Important: harbor buffers all output until the run completes.**
   Do not expect the log file to grow while the task is running — it is
   written in one shot at the end. Use `run_background` + a single
   `wait_for_output` with `timeoutMs` ≥ 1800000 (30 min) and pattern
   `"Mean:|Results written to|Exception"`. Check the result at timeout;
   do not issue follow-up waits.

7. **Full TB2 run** on Sonnet 4.6 (and optionally Opus 4.7) if cost is acceptable.
8. **SWE-Bench Verified** via the same Harbor wrapper — this is the number
   everyone reports. 500 tasks, plan a few hundred dollars of API budget for
   a full pass with Sonnet.
9. Optional: **SWE-Bench Pro** (harder, hasn't saturated) or **Aider
   polyglot** (quick, cheap, multilingual; separate harness though).

## 3. Harbor / Terminal-Bench terminology

Confusing naming because the team evolved the tools. Timeline:

- **May 2025**: Stanford + Laude Institute release Terminal-Bench v1 with its
  own `tb` CLI harness.
- They see the harness used for things beyond just TB (custom evals, RL, SFT,
  CI). Rebuild the harness as a general framework: **Harbor**.
- **Terminal-Bench 2.0** ships as a dataset that runs on Harbor.
- Today: Terminal-Bench 3.0 in development; multiple other benchmarks ported
  into Harbor's registry.

| Term | What it is |
|---|---|
| **Terminal-Bench** | A benchmark — tasks + tests. Versions: 1.x, 2.0, 3.0 (dev). |
| **Terminal-Bench-Core** | The core set of tasks inside Terminal-Bench (as opposed to adapter datasets). |
| **Harbor** | The harness/framework that runs containerized agent benchmarks. General-purpose. |
| **`tb` CLI** | **Legacy** v1 tool. Ignore for anything current. |
| **`harbor` CLI** | Current tool for Harbor / TB 2.0+. |
| **Harbor registry** | Catalog of benchmarks ported onto Harbor — TB 2, SWE-Bench Verified, AppWorld, CompileBench, etc. |
| **harbor-framework (GitHub org)** | Where Harbor and Terminal-Bench live as sibling repos. |

Analogy: **Harbor : Terminal-Bench ≈ pytest : a specific test suite.**

## 4. Integrating Omega into Harbor

Harbor supports two agent types:

- **External** — runs on host, drives container via `exec`. Poor fit.
- **Installed** — installed inside the task container, run headless. ✅ Right fit.

### What Omega needs before wrapping

Currently Omega's only entry point is `bun run src/web/server.ts` (web
server). For benchmarking we need a **headless CLI entrypoint**:

- Takes an instruction string (arg or stdin)
- Accepts a model arg (`--model claude-sonnet-4-6` etc.)
- Runs the agent loop to completion or until turn/cost budget hit
- Exits with `events.jsonl` written to a known session dir
- No TUI, no web server, no interactive prompts

Rough contract:
```bash
omega run --instruction "$INSTR" --model claude-sonnet-4-6 --session-dir /tmp/omega-session
```

All of the above is now implemented in `src/cli.ts`. Version `v0.1.0` is
tagged on `develop`.

### The Harbor-side wrapper

`omega_agent.py` lives at the root of this repo. See the file directly for
the implementation. Key design points:

- `CLI_FLAGS` declares `--max-turns` (int, default 50) and `--effort` (enum,
  default medium); `build_cli_flags()` serialises them to the CLI invocation.
- `self._parsed_model_provider` / `self._parsed_model_name` come from
  `BaseAgent` and split `anthropic/claude-sonnet-4-6` automatically.
- `run()` downloads `events.jsonl` from the container in a `finally` block
  so token data is preserved even on non-zero exit.
- `populate_context_post_run()` reads the downloaded file and sets
  `context.n_input_tokens`, `context.n_output_tokens`, `context.n_cache_tokens`
  from the `turn_end` event's `metrics` object.

Run it from the repo root:
```bash
harbor run -d terminal-bench@2.0 \
  --agent-import-path ./omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY
```

## 5. Model choice

Omega is Anthropic-only (Sonnet 4.6, Opus 4.7). This is **fine** for
benchmarking:

- Leaderboards are agent × model pairs — nobody expects cross-provider support.
- Meaningful comparisons are model-matched (Omega+Sonnet vs Claude Code+Sonnet).
- Claude Code, Terminus-2, Mini-SWE-Agent, and OpenHands all run fine on
  Anthropic models, so head-to-head comparisons are available.
- Avoiding multi-provider abstraction is real engineering savings.

Only thing we can't do: claim the leaderboard slot for "best agent on GPT-5"
or "best agent on Gemini." Not relevant for a comparison study.

**Recommendation:** benchmark on both Sonnet 4.6 and Opus 4.7. Same
scaffolding, two runs — separates scaffolding effects from model strength.

## 6. The "oracle" concept

Each Harbor task ships with a reference `solution.sh`. The **oracle** is a
built-in Harbor agent that just replays that script — no LLM, no API key,
no cost. It works against any Harbor dataset.

Running the oracle first decouples plumbing bugs from agent bugs:

| Layer | Verified by oracle run |
|---|---|
| Docker + Harbor install | ✅ |
| Dataset download / image pull | ✅ |
| Task format, test harness, scoring | ✅ |
| Your agent | ❌ |
| LLM API plumbing | ❌ |

If oracle passes ~100%, the plumbing is good and any later failure is the
agent's problem. This is the first thing to run after installing Harbor.

## 7. Install plan (CachyOS)

### Prerequisites present on this machine

- `uv` 0.5.9, Python 3.14.4, `pipx`, `python` — ✅
- **Docker** — ❌ missing, needs install

### Steps

1. **Install Docker** (sudo required):
   ```bash
   sudo pacman -S --needed docker docker-buildx docker-compose
   # or equivalently: paru -S --needed docker docker-buildx docker-compose
   sudo systemctl enable --now docker.service
   sudo usermod -aG docker "$USER"
   ```
   Then log out/in (or `newgrp docker`) for group membership to take effect.

   **Note:** `docker-compose` is mandatory — it installs the Compose v2 plugin
   (`docker compose`). Without it Harbor's `docker compose --project-name ...`
   calls fail with `unknown flag: --project-name`.

2. **Verify Docker**:
   ```bash
   docker version
   docker compose version   # must show Compose v2.x — not an error
   docker run --rm hello-world
   ```

3. **Install Harbor**:
   ```bash
   uv tool install harbor
   harbor --version
   harbor run --help
   ```

4. **First oracle run** — single task, smoke test:
   ```bash
   harbor run \
     --dataset terminal-bench@2.0 \
     --agent oracle \
     --n-concurrent 2 \
     --no-delete \
     --include-task-name fix-git
   ```
   `fix-git` is ideal: the oracle solution is 5 lines, no builds or downloads,
   runs in seconds. First invocation pulls the Docker image (slow, ~1 min).
   `--no-delete` keeps it cached for subsequent runs.

5. **Small subset**, then full dataset if happy:
   ```bash
   harbor run --dataset terminal-bench@2.0 --agent oracle --n-concurrent 4 --no-delete
   ```
   Budget: ~30–60 GB Docker storage, $0 API cost.

   **`--no-delete` is important.** By default Harbor passes `--rmi all` to
   `docker compose down`, deleting every image after the run. Without
   `--no-delete`, every subsequent run re-pulls all images and takes just as
   long (~2 h on first run, same on every run). With `--no-delete`, images
   stay cached and re-runs drop to ~10 min (execution time only).

### Background: `uv` vs `pipx`

Both install Python CLI tools in isolated venvs. `uv` is newer, Rust-based,
much faster, and subsumes `pipx` (plus `pip`, `venv`, `pyenv`, `poetry`,
etc.). `uv tool install harbor` ≡ `pipx install harbor`. We use `uv` because
it's already on the machine.

### `pacman --needed`

Skips reinstalling packages already at the requested version. Idempotent,
no downside, useful in scripts.

### `paru` vs `pacman`

`paru` is an AUR helper wrapping pacman. For official-repo packages like
`docker`, they're interchangeable. `paru` shines when you need AUR packages
(none here).

## 8. Open questions / things to verify

- ~~Exact Harbor CLI flag names~~ Confirmed: use `--include-task-name` / `-i`
  to filter by task name (supports glob patterns). `--task-ids` does not exist.
- ~~How `self.model` is exposed~~ Confirmed: it's `self.model_name`
  (set by `BaseAgent.__init__`); `self._parsed_model_provider` and
  `self._parsed_model_name` split on `/` automatically.
- Whether ATIF trajectory export is required for leaderboard submission or
  just nice-to-have.
- Whether Omega's `events.jsonl` maps cleanly to ATIF or needs a custom
  translator.
- ~~API cost estimate~~ Approach: run 5–10 tasks, check Anthropic console,
  extrapolate. Pricing reference: Sonnet 4.6 $3/$15 per MTok, Opus 4.7 $5/$25.
- Whether to use Daytona (cloud sandbox, ~32x parallelism) for the full
  benchmarks once plumbing works locally.

## 9. References

- Harbor docs: https://www.harborframework.com/docs/
- Terminal-Bench 2.0 repo: https://github.com/harbor-framework/terminal-bench-2
- Harbor 2.0 + TB announcement: https://www.tbench.ai/news/announcement-2-0
- Registry + SWE-Bench adapter announcement:
  https://www.tbench.ai/news/registry-and-adapters
- Terminal-Bench tutorial on Harbor:
  https://www.harborframework.com/docs/tutorials/running-terminal-bench
- SWE-Bench leaderboards: https://www.swebench.com/
- Example third-party Harbor agent adapter (pi-terminal-bench):
  https://github.com/badlogic/pi-terminal-bench
