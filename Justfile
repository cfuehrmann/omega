# Omega — common workflows
# Run `just --list` to see all recipes.
#
# After Phase 4 there is one production frontend (Leptos, wasm32 via
# Trunk) and one Rust backend (omega-server). The Phase-4 Q7 deletion
# removed the entire TS toolchain (bun, vite, knip, tsconfigs,
# package.json, node_modules, Playwright) — the surviving e2e harness
# is `omega-e2e` (chromiumoxide-driven) and lives at
# crates/omega-e2e.
#
# Production:
#   just server         — builds and starts omega-server on :3000 (Leptos at /)

# Scratch directory for cargo-mutants (keeps per-mutant trees off tmpfs).
mutants-tmp := env('HOME') + "/.cache/cargo-mutants-tmp"

# -----------------------------------------------------------------------
# Private helpers
# -----------------------------------------------------------------------

# Add the wasm32 target and install the matching wasm-bindgen-cli.
# Version-locked: bump both here and in frontends/leptos/Cargo.toml together.
#
# wasm-bindgen-cli: --locked is intentional — it ensures the installed CLI uses
# the same wasm-bindgen-shared ABI as our pinned lib.  The future-incompatibility
# warnings about buf_redux and multipart (pulled in by wasm-bindgen-test-runner
# via rouille) are a known upstream issue (rustwasm/wasm-bindgen#3356) and do
# not affect functionality.
#
# trunk: --locked is intentionally omitted.  trunk@0.21.14's locked Cargo.lock
# pins libdeflate-sys@1.23.1, which uses the 'no-evex512' GCC attribute removed
# in GCC 16.  Omitting --locked lets cargo resolve a newer libdeflate-sys that
# builds on all supported host toolchains.
[private]
wasm-setup:
    rustup target add wasm32-unknown-unknown
    cargo install --locked --version =0.2.121 wasm-bindgen-cli
    cargo install         --version =0.21.14 trunk

# Clippy + cargo test + machete. Assumes dist/ is already built.
# Note: no `cargo fmt --check` here — the pre-commit hook runs `cargo fmt`
# (auto-fix) before the gate, so a check would always be redundant.
[private]
_rust-checks:
    cargo clippy --all-targets -- -D warnings && cargo test
    cargo machete

# Build mock server + run browser tests. Assumes dist/ is already built.
[private]
_rust-e2e-run:
    cargo build --release -p omega-mock-server
    cargo build --release -p omega-server
    cargo test -p omega-e2e --tests -- --ignored --test-threads=1

# -----------------------------------------------------------------------
# Top-level test pipeline
# -----------------------------------------------------------------------

# Full quality gate: Leptos build → test → snapshots → Rust checks → e2e.
# All output is tee'd to test-output/gate-latest.log (overwritten each run).
# On failure, read that file for the complete trace — no need to re-run.
# The Leptos bundle is built exactly once; rust-gate and rust-e2e each
# rebuild it when called standalone.
gate:
    #!/usr/bin/env bash
    set -eo pipefail
    mkdir -p test-output .omega/gate-logs
    TS=$(date -u +"%Y-%m-%dT%H-%M-%S")
    LOG_FILE=".omega/gate-logs/${TS}.log"
    # Keep test-output/gate-latest.log as a backwards-compat symlink so that
    # the pre-commit hook, README references, and CI tooling still find it.
    ln -sf "../.omega/gate-logs/${TS}.log" test-output/gate-latest.log
    BEFORE=$(ls -1 .omega/sessions/ 2>/dev/null | wc -l)
    {
        echo "=== web-leptos-build ==="
        just web-leptos-build
        echo "=== web-leptos-test ==="
        just web-leptos-test
        echo "=== web-leptos-snapshots ==="
        just web-leptos-snapshots
        echo "=== rust-checks ==="
        just _rust-checks
        echo "=== rust-e2e ==="
        just _rust-e2e-run
        echo "=== session-pollution check ==="
        AFTER=$(ls -1 .omega/sessions/ 2>/dev/null | wc -l)
        if [ "$AFTER" -gt "$BEFORE" ]; then
            echo "❌  Tests created $(( AFTER - BEFORE )) session(s) in .omega/sessions/ (production)."
            echo "    Tests must write to .omega/test-sessions/ instead."
            echo "    Before: $BEFORE  After: $AFTER"
            exit 1
        fi
        echo "✅  No production session pollution ($BEFORE sessions before and after)."
        echo "=== done ==="
    } 2>&1 | tee "$LOG_FILE"

# Run the chromiumoxide-driven Rust e2e suite. Builds the Leptos bundle
# and the mock-omega-server fixture binary first, then runs the
# `--ignored` (browser) tests in `omega-e2e`.
rust-e2e: web-leptos-build _rust-e2e-run

# -----------------------------------------------------------------------
# Leptos frontend
# -----------------------------------------------------------------------

# Build the Leptos frontend (trunk → frontends/leptos/dist/).
# Phase 3.7 made this the canonical production bundle; Phase 4 Q7
# flipped Trunk's `public_url` to `/` and omega-server now serves it
# from `/` (the `/leptos/` alias mount is gone).
web-leptos-build: wasm-setup
    cd frontends/leptos && trunk build --release

# Run the Leptos crate's wasm-bindgen-test suite.
# `--lib` is required because the crate is lib + bin (Phase 3.6 split).
# The host-target snapshot harness lives at `tests/snapshots.rs` and
# is gated by `#[cfg(feature = "ssr")]` so it skips here.
web-leptos-test: wasm-setup
    cd frontends/leptos && cargo test --lib --target wasm32-unknown-unknown

# Host-target snapshot harness (TEST-ARCH-5). Renders every component
# at the variant level via leptos's host SSR codepath and snapshots
# the HTML with insta. The `ssr` feature is mutually exclusive with
# `csr`; the bin keeps `csr` (default) and only the snapshot run flips
# features.
web-leptos-snapshots:
    cd frontends/leptos && cargo test --test snapshots --no-default-features --features ssr

# -----------------------------------------------------------------------
# Rust binaries
# -----------------------------------------------------------------------

# Build the production omega-server (release) — target/release/omega-server
rust-build-server:
    cargo build --release -p omega-server

# Build and start the web server (serves the Leptos bundle + WebSocket on :3000).
# Rebuilds the server binary and the Leptos bundle on every invocation.
# Pass any omega-server CLI args, e.g. just server --port 3001
server *args: rust-build-server web-leptos-build
    target/release/omega-server {{args}}

# Show what's listening on :3000.
ports:
    @echo "=== :3000 (omega-server) ===" && lsof -iTCP:3000 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"

# -----------------------------------------------------------------------
# Rust quality gate
# -----------------------------------------------------------------------

# Auto-format all Rust workspaces (rust/ and frontends/leptos/).
# Run this manually any time; the pre-commit hook calls it automatically.
fmt:
    cargo fmt --all
    cd frontends/leptos && cargo fmt
    @echo "✅  All Rust code formatted."

# Rust-only gate: format check + Clippy + cargo test + cargo machete
# + Leptos wasm-bindgen-test suite + Leptos snapshot suite. Runs via
# the pre-commit hook when only rust/ files are staged.
# Run manually: just rust-gate
#
# cargo machete is run from the repo root so it scans *both* the
# root workspace and frontends/leptos/ in one pass. Running it from
# inside a subdirectory would silently skip the other workspace.
rust-gate: web-leptos-build web-leptos-test web-leptos-snapshots _rust-checks

# -----------------------------------------------------------------------
# Mutation testing
# -----------------------------------------------------------------------
#
# `cargo mutants` defaults to `/tmp` for per-mutant scratch trees. On this
# host `/tmp` is tmpfs (≈8 GB) which fills before the sweep finishes;
# redirect to `~/.cache/cargo-mutants-tmp` (real disk). Run sweeps with
# `-j2` to keep peak disk footprint reasonable.

# Run cargo-mutants on the root workspace.
mutants:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -j2

# Run cargo-mutants on the leptos crate (wasm32 target).
web-mutants: wasm-setup
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{mutants-tmp}} cargo mutants -j2 --cargo-arg=--target=wasm32-unknown-unknown

# Run cargo-mutants targeted at the system-prompt-path guard only.
# Mutates only omega-tools/src/lib.rs (where the guard logic lives)
# and runs the fast omega-tools test suite (no network, no subprocesses).
# Fast: typically under 2 minutes on this host.
mutants-system-prompt-guard:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/lib.rs"

# Run cargo-mutants targeted at the identity primitives (Phase 1).
# Mutates only omega-types/src/ids.rs and runs the omega-types test suite.
# Fast: pure functions with no I/O.
mutants-ids:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-types -j2 --cap-lints=true --file "crates/omega-types/src/ids.rs"

# Run cargo-mutants targeted at OmegaEvent (Phase 2.0 — F11).
# Mutates only omega-types/src/events.rs and runs the omega-types test suite.
# Verifies ContextCompacted serialisation, round-trips, and time() accessors.
mutants-events:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-types -j2 --cap-lints=true --file "crates/omega-types/src/events.rs"

# Run cargo-mutants targeted at the canonical tools module in omega-types.
# Tests the tool-name constants, Preset registry, preset_by_id, and all
# pure selection helpers (default_tool_selection / resolve_preset /
# serialize_selection / parse_stored_selection).
# All mutations must be CAUGHT or UNVIABLE — no survivors.
mutants-tools:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-types -j2 --cap-lints=true --file "crates/omega-types/src/tools.rs"

# Run cargo-mutants targeted at the Phase 0 context projection logic.
# Mutates agent.rs (project_messages, monitor injection methods, and the
# XML-wrapper formatters format_monitor_lines / format_monitor_stopped that
# emit <monitor id="…">…</monitor> / <monitor-stopped …/> — the framing that
# prevents mis-attribution and fabrication of monitor output) and runs the
# full omega-agent test suite including the format_monitor unit tests and
# the Phase 0 monitor projection tests.
# Template: mutants-system-prompt-guard (see AGENTS.md).
mutants-agent-projection:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true --file "crates/omega-agent/src/agent.rs"

# Run cargo-mutants targeted at the Phase 4 shutdown-logging logic.
# Scoped to format_monitor_lines, format_monitor_stopped, and
# shutdown_and_log_monitors in agent.rs.  Uses --in-place to avoid
# copying the large target directory (6 GB) to TMPDIR.
mutants-agent-shutdown:
    cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        -F "shutdown_and_log_monitors|format_monitor_stopped|format_monitor_lines"

# Run cargo-mutants targeted at the strict-resume fold logic (Phase 2.1-2.4).
# Mutates session_resume.rs (resumable-boundary predicate, context-hash
# reconstruction, model/effort folding, strict event reader).
# Uses omega-agent's full test suite including the round_trip_gate test.
mutants-strict-resume:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true --file "crates/omega-agent/src/session_resume.rs"

# Run cargo-mutants targeted at the domain-snapshot type and related logic
# (Phase 2 follow-up + features correction): DomainSnapshot,
# Agent::domain_snapshot, fold_system_prompt, fold_features,
# and Agent::init_for_resume.
# Uses --in-diff to restrict to code changed since main, keeping the run
# fast.  Uses omega-agent's full test suite including the updated
# round_trip_gate which now exercises non-default feature flags.
mutants-domain-snapshot:
    mkdir -p {{mutants-tmp}}
    HOME=/tmp git --no-pager diff HEAD~3..HEAD -- \
        crates/omega-agent/src/agent.rs \
        crates/omega-agent/src/session_resume.rs \
        > /tmp/omega-domain-snapshot.diff
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true \
        --in-diff /tmp/omega-domain-snapshot.diff

# Run cargo-mutants targeted at the feature-flag parsing module.
# Mutates omega-types/src/feature_flags.rs and runs the omega-types test suite.
# Covers parse_flag_value / from_values for the `subagents` flag;
# from_env is excluded via #[mutants::skip].
mutants-feature-flags:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-types -j2 --cap-lints=true --file "crates/omega-types/src/feature_flags.rs"

# Run cargo-mutants targeted at the stateful Python REPL module
# (PythonRepl::execute truncation logic, sentinel handling, output collection).
# Spawns real python3 subprocesses — requires python3 in $PATH.
# After the 2025-11 file split, the module is a directory with one submodule
# per concern; we sweep the whole tree.
mutants-python-repl:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true \
        --file "crates/omega-tools/src/python_repl.rs" \
        --file "crates/omega-tools/src/python_repl/*.rs"

# Run cargo-mutants targeted at the shared process-kill helpers (kill_group, kill_soft).
# Both functions route through the shell; mutations that swap SIGKILL for SIGINT or
# alter the negated-pgid sign are caught by the timeout integration tests.
mutants-process-util:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/process_util.rs"

# Run cargo-mutants targeted at the async-monitor runtime (Monitors Phase 1).
# Mutates the MonitorManager (spawn / stop / shutdown / queue + roster
# mutations) and runs the omega-tools suite incl. the 9 monitor E2E tests.
# Spawns real bash subprocesses (printf / sleep / seq) — requires bash.
# Template: mutants-process-util (see AGENTS.md).
mutants-monitors:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/monitors.rs"

# Run cargo-mutants targeted at the two monitor tool wrappers (Monitors Phase 1):
# monitor() (spawn + MonitorStarted extra_event) and stop_monitor() (kill +
# MonitorStopped/StoppedByAgent extra_event, no-op on unknown/dead). Exercised
# via execute_tool in the monitor E2E tests. Template: mutants-process-util.
mutants-monitor-tools:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true \
        --file "crates/omega-tools/src/tools/monitor.rs" \
        --file "crates/omega-tools/src/tools/stop_monitor.rs"

# Run cargo-mutants targeted at the python3 bootstrap logic in python_repl.rs.
# Covers is_not_found(), start_inner() branching (AptNotFound / AptFailed /
# Succeeded), retry logic, and the BootstrapInfo return path.
# bootstrap_python3() and run_apt_get() are marked #[mutants::skip] because
# they call real OS processes; the logic branches they implement are exercised
# via mock-closure unit tests in start_inner.
mutants-python-repl-bootstrap:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true \
        --file "crates/omega-tools/src/python_repl/bootstrap.rs"

# Run cargo-mutants targeted at the REPL resume guard in session_resume.rs.
# Verifies that the ReplResumeUnsupported check cannot be mutated away.
# Template: mutants-system-prompt-guard (see AGENTS.md).
mutants-repl-resume:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true --file "crates/omega-agent/src/session_resume.rs"

# Run cargo-mutants targeted at the absolute sessions-root resolution used by
# GET /api/sessions, so the picker's "Copy @path" button yields an absolute
# reference. Scoped to `absolute_sessions_root` (relative-default anchoring,
# absolute-root passthrough) and `list_sessions` (per-item `path`). The
# integration assertion lives in tests/http.rs::get_sessions_item_path_is_absolute.
mutants-sessions-root:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-server -j2 --cap-lints=true \
        --file "crates/omega-server/src/router.rs" --re 'absolute_sessions_root|list_sessions'

# §15 U1 (Unified Input Model) — the persistent per-session agent loop.
# Scoped to Agent::run + Agent::drive_turn in agent.rs.  Both are `stream!`
# macro generators, so cargo-mutants can only mutate the outer fn body (the
# macro body is opaque); the body-replacement mutants come back UNVIABLE
# (`Default::default()` is not implemented for `Pin<Box<dyn Stream>>`).
# Uses --in-place to avoid copying the 6 GB target directory to TMPDIR.
mutants-agent-run-loop:
    cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'Agent::run|Agent::drive_turn'

# §14 — Empty-response continuation (documented Anthropic behaviour).
# Covers the EMPTY_RESPONSE_CAP check, continuation injection, and the
# empty-block detection in Agent::drive_turn.
# Both Agent::run and Agent::drive_turn are `stream!` macro generators,
# so cargo-mutants can only mutate the outer fn body (the macro body is
# opaque); inner-body mutations come back UNVIABLE
# (`Default::default()` not implemented for `Pin<Box<dyn Stream>>`).
# Uses --in-place to avoid copying the 6 GB target directory.
mutants-empty-response:
    cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'drive_turn'

# §15 HarnessRecovery event type — new OmegaEvent::HarnessRecovery variant, the
# HarnessRecoveryKind enum, HarnessRecoveryEvent struct, and time() accessor.
# Mutates omega-types/src/events.rs and runs the omega-types test suite.
# All mutations must be CAUGHT or UNVIABLE.
mutants-harness-recovery-events:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-types -j2 --cap-lints=true --file "crates/omega-types/src/events.rs"

# §15 inject_harness_recovery helper — the free method on Agent that
# emits the HarnessRecovery event + appends to context/history.
# The call sites inside the async_stream! macro report UNVIABLE
# (stream generators are opaque to cargo-mutants); the helper itself,
# being a plain async method, should have its mutations CAUGHT.
# Uses --in-place to avoid copying the 6 GB target directory.
mutants-harness-recovery-agent:
    cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'inject_harness_recovery'

# §15(a) A1 — the three inject_* helpers introduced by A1:
# inject_user_message, inject_dangling_tool_results, inject_tool_results_batch.
# The call sites inside the async_stream! macro body report UNVIABLE
# (stream generators are opaque to cargo-mutants); the helpers themselves,
# being plain async methods, have their body-replacement mutations CAUGHT.
# The guard test (user_role_context_appends_are_event_backed) is a
# string-scan assertion and is not mutation-testable; that is documented
# in the test's comment in tests/internal.rs.
# Uses --in-place to avoid copying the 6 GB target directory.
mutants-a1-inject-helpers:
    cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'inject_user_message|inject_dangling_tool_results|inject_tool_results_batch'

# §15 U1 — the server glue for the persistent run task: handle_user_message
# (now just inbox.send), spawn_run_task (owns the agent lock + forwards the
# run stream to WS, incl. turn-state + roster pushes), and teardown_prior_run
# (abort + run_cancel + join + session-end monitor reap).  Exercised by the
# two_sequential_user_messages_share_one_run_task and
# reset_reaps_prior_sessions_live_monitor tests in tests/ws_router.rs.
mutants-server-run-task:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-server -j2 --cap-lints=true \
        --file "crates/omega-server/src/router.rs" \
        --re 'handle_user_message|spawn_run_task|teardown_prior_run'

# Run cargo-mutants targeted at the picker's `session_at_path` formatter, which
# wraps the server-supplied absolute path as an `@<path>/` composer reference.
# Runs on the wasm target (the only one the leptos crate's tests build for).
mutants-session-at-path:
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{mutants-tmp}} cargo mutants -j2 \
        --cargo-arg=--target=wasm32-unknown-unknown --cap-lints=true \
        --file "src/picker.rs" --re 'session_at_path'

# Run cargo-mutants targeted at the context-modal `render_block` projection,
# which formats one content block (text / tool_use / tool_result / thinking)
# to its display string — including the inline tool-id labels that let a
# tool_use and its tool_result be paired by the protocol's opaque id.
# Runs on the wasm target (the only one the leptos crate's tests build for).
mutants-render-block:
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{mutants-tmp}} cargo mutants -j2 \
        --cargo-arg=--target=wasm32-unknown-unknown --cap-lints=true \
        --file "src/context_modal.rs" --re 'render_block'

# Run cargo-mutants targeted at the schemas.rs tool-definition filtering.
# Covers the tool_definitions(tool_selection) membership-driven filtering,
# canonical-order iteration, and the shell-aware fetch_url schema branch.
mutants-schemas:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/schemas.rs"

# Run cargo-mutants targeted at the fetch_url tool implementation.
# Covers the shell-aware branch (driven by shell-tool presence in
# tool_selection), the postprocess path, and the apply_shell_gated_cap
# truncation logic.
# Requires network access (real HTTP fetches to example.com).
mutants-fetch-url:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/tools/fetch_url.rs"

# Run cargo-mutants targeted at the system_prompt.rs block assembly.
# Covers file-tool-absent and shell-tool-absent branches (driven by
# tool_selection membership), the python_repl addendum, and the combined
# reduced_toolset_addendum.
mutants-system-prompt:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true --file "crates/omega-agent/src/system_prompt.rs"

# Phase 1.2 — the three files most changed when REPL feature flags were
# replaced with `SessionStartedEvent.tool_selection`:
#
#   * `feature_flags.rs`  — now exposes only `subagents`.
#   * `schemas.rs`        — owns `DEFAULT_TOOL_NAMES` / `ALL_TOOL_NAMES`
#                           and the membership-driven `tool_definitions`.
#   * `system_prompt.rs`  — derives `has_file_tools`, `has_shell_tools`,
#                           `has_python_repl` from the selection.
#
# Mutations on any of these would silently break the new contract — every
# mutation must end up *caught* or *unviable*.  Run this recipe whenever
# you touch the toolset wiring.
mutants-tool-selection: mutants-feature-flags mutants-schemas mutants-system-prompt

# Phase 2.2.1 — timeout over-cap rejection in the python_repl dispatch arm.
# Covers the new over-cap rejection path added to execute_tool's python_repl arm.
mutants-python-repl-timeout:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true --file "crates/omega-tools/src/lib.rs"

# Phase 2.2.1 — full python_repl module sweep (includes repl.rs constant change).
# Covers MAX_TIMEOUT_SECS constant and repl.execute() defence-in-depth clamp.
mutants-python-repl-221:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-tools -j2 --cap-lints=true \
        --file "crates/omega-tools/src/python_repl/repl.rs"

# Phase 2.2.1 — system_prompt.rs: timeout constants + sh() / SyntaxWarning.
# Alias for mutants-system-prompt scoped to the 2.2.1 additions.
mutants-system-prompt-221:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true --file "crates/omega-agent/src/system_prompt.rs"

# Nudging revamp — system_prompt.rs: monitor_addendum behavioral rules.
# Scoped to monitor_addendum: verifies that mutations weakening the
# not-the-user / don't-fabricate / end-turn-and-wait rules are caught by
# the monitor_addendum_contains_* tests.
mutants-monitor-addendum:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-agent -j2 --cap-lints=true \
        --file "crates/omega-agent/src/system_prompt.rs" --re 'monitor_addendum'

# Phase 2.3 — event_view.rs: python_repl arm in tool_call_preview.
# Must be run from the frontends/leptos directory since omega-web is
# a standalone workspace.  All mutations must be caught or unviable.
mutants-python-repl-23:
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{justfile_directory()}}/{{mutants-tmp}}         cargo mutants --cap-lints=true         --file "src/event_view.rs"

# Phase 3 (UI: roster badge + modal) — is_monitor_event and roster_snapshot_msg
# in router.rs.  These are the two non-trivial decision functions that govern
# (a) WHICH events trigger a follow-up roster push, and (b) HOW MonitorInfo is
# projected into the MonitorRosterItem wire format.  All mutations must be
# caught or unviable; the connect-time and per-event WS tests + router.rs unit
# tests provide coverage.
mutants-monitor-roster-push:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-server -j2 --cap-lints=true \
        --file "crates/omega-server/src/router.rs" --re 'is_monitor_event|roster_snapshot_msg'

# Phase 3 (UI: roster badge + modal) — MonitorRoster serialisation in
# ws_message.rs.  NOTE: cargo-mutants finds 0 mutants here because
# serde_json::json!{...} is a macro call, not a regular function body.
# Coverage is provided instead by the ws_message unit tests that snapshot
# the exact JSON output (type field, monitors array, every item field);
# any change to the wire format immediately breaks those tests.
# This recipe is kept as documentation of the decision.
mutants-monitor-ws-message:
    mkdir -p {{mutants-tmp}}
    TMPDIR={{mutants-tmp}} cargo mutants -p omega-server -j2 --cap-lints=true \
        --file "crates/omega-server/src/ws_message.rs" --re 'MonitorRoster|monitor_roster'

# Phase 5 — monitors_panel.rs: badge_label + running_count + total_fired.
# These three pure derivation functions carry the mutation-test budget for the
# monitors panel (the view/component body itself is #[mutants::skip]).
# Tests: wasm-bindgen-test unit tests in monitors_panel.rs + snapshot tests.
mutants-monitors-panel:
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{mutants-tmp}} cargo mutants -j2 --cap-lints=true \
        --file "src/monitors_panel.rs" \
        --no-default-features --features ssr

# Phase 2.3 — feed.rs: ToolUseBlock dispatch (timeout chip + expansion body).
# Scoped to the ToolUseBlock-related logic via --regex to keep runtime
# manageable.  feed.rs is otherwise mostly JS-interop glue exempt from
# mutation testing (see component docs in feed.rs).
mutants-feed-tool-use-23:
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{justfile_directory()}}/{{mutants-tmp}}         cargo mutants --cap-lints=true         --file "src/feed.rs"         --regex "PYTHON_REPL_DEFAULT_TIMEOUT_SECS|timeout_chip|python_repl"

# -----------------------------------------------------------------------
# Repo housekeeping
# -----------------------------------------------------------------------

# Tag the current commit with the version declared in omega_agent.py and push
# both the tag and the current branch to origin.
release:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(grep -m1 'OMEGA_VERSION' bench/omega_agent.py | sed 's/.*"\(.*\)".*/\1/')
    if [ -z "$VERSION" ]; then
        echo "❌  Could not read OMEGA_VERSION from bench/omega_agent.py" >&2
        exit 1
    fi
    if git rev-parse "$VERSION" >/dev/null 2>&1; then
        echo "❌  Tag $VERSION already exists. Bump OMEGA_VERSION in omega_agent.py first." >&2
        exit 1
    fi
    git push
    git tag "$VERSION"
    git push origin "$VERSION"
    echo "✅  Released $VERSION"

# Install git hooks (pre-commit test gate)
install-hooks:
    cp scripts/pre-commit .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    @echo "✅  Git hooks installed."

# §15 U1 — mutation-test the InputQueue logic (push/pop/snapshot).
# All mutations must be CAUGHT or UNVIABLE; a survivor means a test gap.
mutants-input-queue:
    mkdir -p {{mutants-tmp}}
    cargo mutants -p omega-agent --cap-lints=true \
        --file "crates/omega-agent/src/input_queue.rs" \
        -- --tmp-dir {{mutants-tmp}}

# §15 U1 — mutation-test the server push decision points in router.rs.
# Target: `is_user_message_event`, `queue_snapshot_msg`, enqueue/drain push hooks.
mutants-input-queue-router:
    mkdir -p {{mutants-tmp}}
    cargo mutants -p omega-server --cap-lints=true \
        --file "crates/omega-server/src/router.rs" \
        -- --tmp-dir {{mutants-tmp}}
