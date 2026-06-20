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

# Mode split (measured 2026-06-20): the targeted per-file recipes run
# `--in-place` — fastest in every regime because it reuses the warm build cache
# (in-place ties or beats copy `-j1`/`-j2` everywhere; the cold per-run baseline
# build, not the tree copy, was always the gap). In-place forces sequential
# (-j1) and never runs two sweeps at once; that costs nothing since `-j>1` was a
# noisy non-win. Only a handful stay COPY mode: the long unattended full sweeps
# (`mutants`, `web-mutants`) where crash-safety outweighs speed and the copy
# amortises over thousands of mutants, `mutants-bench`, and the one recipe with
# a scratch-dir test dependency (`mutants-input-queue-router`).
#
# Disk hygiene for those COPY-mode sweeps: wipe the scratch dir at the START.
# cargo-mutants self-cleans on a normal finish, but an interrupted sweep leaves
# per-worker target copies behind and a trailing cleanup would be skipped on
# exactly that path. Cleaning before bounds disk to one sweep's footprint. The
# in-place recipes don't use this — they're guarded by `{{mutants-guard}}`.
mutants-prep := "rm -rf " + mutants-tmp + " && mkdir -p " + mutants-tmp

# Memory cap for cargo-mutants runs. A single pathological mutant can turn a
# bounded loop into an unbounded one (e.g. `i += 1` -> `i *= 1` pins the index
# and an allocating loop never terminates) and balloon a test process to tens
# of GB. Unconstrained, that triggers the GLOBAL OOM killer, which takes down
# the whole terminal/session — exactly how the 2026-06-19 session died. Running
# every sweep inside a transient systemd scope confines the blast radius. Three
# properties make it work cleanly (all verified empirically, 2026-06-20):
#   MemoryMax=20G   — cgroup hard cap; far above any legitimate -j1 build (a
#                     few GB), well below the ~30 GB global-OOM cliff. The
#                     runaway hits this and the KERNEL OOM-kills just that one
#                     process (memory.oom.group is 0 by default, so siblings
#                     survive). Because the cap is cgroup-bounded, the global
#                     system never OOMs no matter how wild the mutant.
#   MemorySwapMax=0 — no swap, so the runaway dies in seconds instead of
#                     thrashing tens of GB of swap (what made the crash slow).
#   OOMPolicy=continue — THE key flag. By default systemd marks a scope
#                     `Failed` and SIGTERMs the survivors when any process is
#                     OOM-killed; that tears down cargo-mutants too and the
#                     sweep aborts with `interrupted`. `continue` tells systemd
#                     to just log it, so cargo-mutants keeps running, observes
#                     the test process killed by signal, and records the mutant
#                     as CAUGHT — naming it in the normal output. The sweep
#                     completes; no perf cost on non-runaway mutants.
# The trailing `env` lets each recipe carry its inline `TMPDIR=…` assignment.
mutants-mem-cap := "systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 -p OOMPolicy=continue --quiet env"

# Clean-tree guard, run first in every in-place mutation recipe: aborts if any
# tracked file is dirty, so a hard crash mid-sweep can't leave a stray mutation
# that gets silently committed. Bypass with MUTANTS_ALLOW_DIRTY=1. See the
# script for the full rationale. (Normal recipes are NOT guarded — they must
# work on uncommitted edits.)
mutants-guard := "scripts/mutants-guard"

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

# Clippy + tests + machete. Assumes dist/ is already built.
# Note: no `cargo fmt --check` here — the pre-commit hook runs `cargo fmt`
# (auto-fix) before the gate, so a check would always be redundant.
#
# Test runner: prefer `cargo nextest` when installed — it runs tests in one
# global pool ACROSS all ~41 test binaries, where plain `cargo test` runs
# binaries one at a time (parallel only within a binary). Measured ~1.7x
# faster on a 4-core slice (≈13s → ≈7.7s; bigger on the full 16 cores), with
# an identical 1089-test set passing (the workspace has no runnable doctests,
# so nextest drops nothing). Cross-binary parallelism is safe here: every
# server/mock test binds 127.0.0.1:0 (ephemeral ports).
#
# Reversible by design: set OMEGA_TEST_RUNNER to force a command (e.g.
# `cargo test`), or just uninstall nextest — this falls back to `cargo test`
# automatically. The e2e step (_rust-e2e-run) has its OWN nextest wiring
# (separate concurrency knob) rather than sharing this one, because browser
# tests need a bounded -j8, not the default all-cores pool.
[private]
_rust-checks:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --all-targets -- -D warnings
    runner="${OMEGA_TEST_RUNNER:-}"
    if [ -z "$runner" ]; then
        if command -v cargo-nextest >/dev/null 2>&1; then
            runner="cargo nextest run"
        else
            runner="cargo test"
        fi
    fi
    echo "rust-checks: test runner = ${runner}"
    ${runner}
    cargo machete

# Build mock server + run browser tests. Assumes dist/ is already built.
#
# Parallel e2e: every test is fully isolated (ephemeral 127.0.0.1:0 ports,
# TempDir sessions root, own mock-server subprocess, AND its own Chromium
# --user-data-dir), so the suite is parallel-safe. With nextest we run it
# in parallel across all 11 test binaries: ~3x faster than serial on a
# 16-core box (50s -> 16s). -j8 is the knee of the curve -- higher just
# saturates cores for <1s gain and raises flake risk on a loaded machine.
# Override with OMEGA_E2E_TEST_THREADS. Without nextest, fall back to the
# original serial `cargo test` (also correct, just slow).
[private]
_rust-e2e-run:
    #!/usr/bin/env bash
    set -euo pipefail
    # One invocation for both release binaries so cargo resolves features
    # once (separate `-p` builds re-unify features and rebuild twice).
    cargo build --release -p omega-mock-server -p omega-server
    threads="${OMEGA_E2E_TEST_THREADS:-8}"
    if command -v cargo-nextest >/dev/null 2>&1; then
        echo "rust-e2e: cargo nextest run (--test-threads ${threads})"
        cargo nextest run -p omega-e2e --run-ignored ignored-only --test-threads "${threads}"
    else
        echo "rust-e2e: cargo test serial fallback (install cargo-nextest for parallel)"
        cargo test -p omega-e2e --tests -- --ignored --test-threads=1
    fi

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
    # `.omega/sessions` must exist before the BEFORE/AFTER pollution count below:
    # on a fresh checkout it doesn't, so `ls .omega/sessions/` fails, and under
    # `set -eo pipefail` that exit code aborts the whole gate before any build
    # runs (silent: the error is hidden by `2>/dev/null`). Create it up front.
    mkdir -p test-output .omega/gate-logs .omega/sessions
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
# host `/tmp` is tmpfs (16 GB) which fills before the sweep finishes;
# redirect to `~/.cache/cargo-mutants-tmp` (real disk, 1.8 TB free).
#
# Parallelism: every sweep runs at `-j1`. This is EVIDENCE-BASED, not a
# headroom guess. Measured on this 16-core box (mutants phase, wall):
#   omega-agent conv_state.rs (30 mutants): -j1 53s, -j2 69s, -j4 87s, -j8 126s
#   omega-types tools.rs      (18 mutants): -j1 13s, -j2 12s, -j4 14s, -j8 23s
# i.e. `-j` is flat-to-worse across the whole crate-weight spectrum and -j8 is
# up to 2.4x SLOWER. Each mutant's own `cargo build`+test already saturates all
# 16 cores (rustc/codegen/link are internally parallel; cargo-mutants' jobserver
# caps total build tasks at NCPUS), so adding cross-mutant concurrency only
# oversubscribes — and in copy mode pays redundant per-worker rebuilds on top.
# An earlier `-j2 -> -j4` bump (commit 3f93e65) was a 1.6x pessimization; this
# reverts it to the measured optimum. `--in-place` recipes are sequential by
# nature (they share the one source tree) and so carry no `-j` either.
#
# Every invocation is wrapped in `{{mutants-mem-cap}}` (see its definition near
# the top of this file) so a runaway-loop mutant is OOM-killed inside its own
# cgroup instead of triggering a global OOM that kills the whole session.

# Run cargo-mutants on the root workspace.
mutants:
    {{mutants-prep}}
    {{mutants-mem-cap}} TMPDIR={{mutants-tmp}} cargo mutants -j1

# Run cargo-mutants on the leptos crate (wasm32 target).
web-mutants: wasm-setup
    {{mutants-prep}}
    cd frontends/leptos && {{mutants-mem-cap}} TMPDIR={{mutants-tmp}} cargo mutants -j1 --cargo-arg=--target=wasm32-unknown-unknown

# Run cargo-mutants targeted at the retry/backoff loop (retry.rs).
# Covers compute_backoff (Retry-After vs. exponential), the give-up
# predicate (max_attempts: Option<u32> — None = retry indefinitely), and
# the LlmRetry event construction. Exercised via tests/retry.rs through a
# real AnthropicProvider + wiremock. Template: mutants-system-prompt-guard.
# Uses --in-place to avoid copying the large (incl. leptos wasm) target tree.
mutants-retry:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-core --in-place --cap-lints=true --file "crates/omega-core/src/retry.rs"

# Run cargo-mutants targeted at LlmError::is_retryable / retry_after / status
# in types.rs. Verifies the "any 5xx + 429 (minus long-context) is retryable"
# classification that decides which "Anthropic can't serve" situations are
# retried. Exercised via tests/retry.rs. Template: mutants-system-prompt-guard.
# Uses --in-place to avoid copying the large (incl. leptos wasm) target tree.
mutants-llm-error-retryable:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-core --in-place --cap-lints=true \
        --file "crates/omega-core/src/types.rs" \
        -F "is_retryable|retry_after|fn status"

# cargo-mutants on the SSE streaming decoder (stream_impl in anthropic.rs).
#
# CARVE-OUT: stream_impl's entire body lives inside an `async_stream::try_stream!`
# macro, which cargo-mutants treats as an opaque token stream and does NOT
# descend into — so this run reports "0 mutants found" (verify with --list).
# The truncated-stream guard (a 200 whose SSE body ends at EOF without
# `message_stop` → retryable Transport error, via the `saw_message_stop` flag
# + post-loop check) therefore cannot be mutation-tested. Its coverage is
# instead proven by the red/green pair in tests/anthropic.rs:
# `empty_stream_maps_to_transport_error` and
# `partial_stream_without_message_stop_maps_to_transport_error` — both fail
# when the guard condition is neutralised and pass with it in place.
# The recipe is kept so the 0-mutants result is documented and re-checkable.
# Uses --in-place to avoid copying the large (incl. leptos wasm) target tree.
mutants-anthropic-stream:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-core --in-place --cap-lints=true \
        --file "crates/omega-core/src/anthropic.rs" -F "stream_impl" --list

# Run cargo-mutants targeted at the system-prompt-path guard only.
# Mutates only omega-tools/src/lib.rs (where the guard logic lives)
# and runs the fast omega-tools test suite (no network, no subprocesses).
# Fast: typically under 2 minutes on this host.
mutants-system-prompt-guard:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/lib.rs"

# Run cargo-mutants targeted at the seam-typestate conversation-shape guard.
# Mutates conv_state.rs (conv_state, classify_move, next_state — the δ that
# keeps the in-memory conversation a valid Anthropic sequence, incl. the
# tool_use↔tool_result id bijection) and runs the full omega-agent suite.
# All mutations must be CAUGHT or UNVIABLE — no survivors.
# Uses --in-place to avoid copying the large (incl. leptos wasm) target tree.
mutants-conv-state:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/agent/conv_state.rs"

# Run cargo-mutants targeted at the identity primitives (Phase 1).
# Mutates only omega-types/src/ids.rs and runs the omega-types test suite.
# Fast: pure functions with no I/O.
mutants-ids:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-types --in-place --cap-lints=true --file "crates/omega-types/src/ids.rs"

# Run cargo-mutants targeted at OmegaEvent (Phase 2.0 — F11).
# Mutates only omega-types/src/events.rs and runs the omega-types test suite.
# Verifies ContextCompacted serialisation, round-trips, and time() accessors.
mutants-events:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-types --in-place --cap-lints=true --file "crates/omega-types/src/events.rs"

# Run cargo-mutants targeted at the canonical tools module in omega-types.
# Tests the tool-name constants, Preset registry, preset_by_id, and all
# pure selection helpers (default_tool_selection / resolve_preset /
# serialize_selection / parse_stored_selection).
# All mutations must be CAUGHT or UNVIABLE — no survivors.
mutants-tools:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-types --in-place --cap-lints=true --file "crates/omega-types/src/tools.rs"

# Find the optimal -j (cross-mutant parallelism) for a sweep BY MEASURING it,
# instead of guessing. Times the SAME mutant set at each -j level and reports a
# winner. The optimum is regime-dependent (see docs/performance-improvements.html
# #mutants): build-bound crates → -j1 (copy mode duplicates the CPU-heavy build
# per worker); latency-bound crates whose tests WAIT on I/O — subprocesses,
# sockets, monitors, timeouts (omega-tools, and any browser/e2e regime) → ~ -j2
# fills the idle cores, but the peak is shallow and regresses by -j4.
#
#   just mutants-bench omega-tools crates/omega-tools/src/format.rs
#   just mutants-bench omega-agent crates/omega-agent/src/conv_state.rs "1 2 4 8"
#
# Big file? Restrict to a deterministic subset for speed via the 4th arg
# (shard COUNT → runs shard 1/COUNT, the SAME ~1/COUNT mutants at every -j, so
# the relative ranking is preserved):
#   just mutants-bench omega-server crates/omega-server/src/ws_message.rs "1 2 4" 4
#
# Close call? Pass a 5th arg reps=N to run each level N times and keep the MIN
# wall (min is least polluted by background interference):
#   just mutants-bench omega-types crates/omega-types/src/tools.rs "1 2" "" 3
#
# Wall-clock is the decision metric. The "N mutants tested in Ts" summary is the
# completion signal, so a sweep with surviving (MISSED) mutants still benchmarks
# fine; a level that ERRORS (e.g. ENOSPC) is reported and excluded from the winner.
# A win is only declared when the fastest level beats -j1 by >10% (else noise → -j1).
#
# Measure optimal -j for a sweep: `just mutants-bench PKG FILE [LEVELS] [SHARD] [REPS]`
mutants-bench pkg file levels="1 2 4" shard="" reps="1":
    #!/usr/bin/env bash
    set -uo pipefail
    shard_arg=""
    [ -n "{{shard}}" ] && shard_arg="--shard 1/{{shard}}"
    echo "mutants-bench: {{file}}  (pkg {{pkg}})  levels=[{{levels}}]  reps={{reps}}  ${shard_arg:-full}"
    echo
    best_j=""; best_t=""; j1_t=""
    for J in {{levels}}; do
        jt=""; summary=""; err=""
        for r in $(seq 1 {{reps}}); do
            {{mutants-prep}}
            log="/tmp/mutants-bench-{{pkg}}-j$J-r$r.log"
            start=$(date +%s.%N)
            {{mutants-mem-cap}} TMPDIR={{mutants-tmp}} cargo mutants -p {{pkg}} -j"$J" $shard_arg \
                --cap-lints=true --file "{{file}}" >"$log" 2>&1 || true
            end=$(date +%s.%N)
            t=$(awk "BEGIN{printf \"%.1f\", $end-$start}")
            if grep -q "mutants tested" "$log"; then
                summary=$(grep 'mutants tested' "$log" | tail -1)
                # keep the MIN wall across reps (least polluted by interference)
                if [ -z "$jt" ] || awk "BEGIN{exit !($t < $jt)}"; then jt=$t; fi
            else
                err=$(grep -iE "error|no space|failed|interrupted" "$log" | tail -1)
            fi
        done
        if [ -n "$jt" ]; then
            printf "  -j%-2s  %7ss   %s\n" "$J" "$jt" "$summary"
            [ "$J" = "1" ] && j1_t=$jt
            if [ -z "$best_t" ] || awk "BEGIN{exit !($jt < $best_t)}"; then best_t=$jt; best_j=$J; fi
        else
            printf "  -j%-2s  ERRORED — %s  (see /tmp/mutants-bench-{{pkg}}-j%s-r*.log)\n" "$J" "${err:-unknown}" "$J"
        fi
    done
    echo
    if [ -z "$best_j" ]; then
        echo "No level completed — inspect /tmp/mutants-bench-{{pkg}}-j*.log"
    elif [ -n "$j1_t" ] && [ "$best_j" != "1" ] && awk "BEGIN{exit !($best_t < 0.90*$j1_t)}"; then
        printf "Winner: -j%s (%ss) beats -j1 (%ss) by >10%% — a real win. Set -j%s for {{file}}.\n" "$best_j" "$best_t" "$j1_t" "$best_j"
    elif [ -n "$j1_t" ]; then
        printf "Keep -j1: fastest was -j%s (%ss), within ~10%% of -j1 (%ss) = noise. Re-run with higher reps= to confirm a close call.\n" "$best_j" "$best_t" "$j1_t"
    else
        printf "Fastest: -j%s (%ss) (no -j1 baseline in LEVELS). Set -j%s for {{file}}.\n" "$best_j" "$best_t" "$best_j"
    fi

# Run cargo-mutants targeted at the Phase 0 context projection logic.
# Mutates agent.rs (project_messages, monitor injection methods, and the
# XML-wrapper formatters format_monitor_lines / format_monitor_stopped that
# emit <monitor id="…">…</monitor> / <monitor-stopped …/> — the framing that
# prevents mis-attribution and fabrication of monitor output) and runs the
# full omega-agent test suite including the format_monitor unit tests and
# the Phase 0 monitor projection tests.
# Template: mutants-system-prompt-guard (see AGENTS.md).
mutants-agent-projection:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/agent.rs"

# Run cargo-mutants targeted at the Phase 4 shutdown-logging logic.
# Scoped to format_monitor_lines, format_monitor_stopped, and
# shutdown_and_log_monitors in agent.rs.  Uses --in-place to avoid
# copying the large target directory (6 GB) to TMPDIR.
mutants-agent-shutdown:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        -F "shutdown_and_log_monitors|format_monitor_stopped|format_monitor_lines"

# Run cargo-mutants targeted at the strict-resume fold logic (Phase 2.1-2.4).
# Mutates session_resume.rs (resumable-boundary predicate, context-hash
# reconstruction, model/effort folding, strict event reader).
# Uses omega-agent's full test suite including the round_trip_gate test.
mutants-strict-resume:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/session_resume.rs"

# Run cargo-mutants targeted at the domain-snapshot type and related logic
# (Phase 2 follow-up + features correction): DomainSnapshot,
# Agent::domain_snapshot, fold_system_prompt, fold_features,
# and Agent::init_for_resume.
# Uses --in-diff to restrict to code changed since main, keeping the run
# fast.  Uses omega-agent's full test suite including the updated
# round_trip_gate which now exercises non-default feature flags.
mutants-domain-snapshot:
    {{mutants-guard}}
    HOME=/tmp git --no-pager diff HEAD~3..HEAD -- \
        crates/omega-agent/src/agent.rs \
        crates/omega-agent/src/session_resume.rs \
        > /tmp/omega-domain-snapshot.diff
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true \
        --in-diff /tmp/omega-domain-snapshot.diff

# Run cargo-mutants targeted at the feature-flag parsing module.
# Mutates omega-types/src/feature_flags.rs and runs the omega-types test suite.
# Covers parse_flag_value / from_values for the `subagents` flag;
# from_env is excluded via #[mutants::skip].
mutants-feature-flags:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-types --in-place --cap-lints=true --file "crates/omega-types/src/feature_flags.rs"

# Run cargo-mutants targeted at the stateful Python REPL module
# (PythonRepl::execute truncation logic, sentinel handling, output collection).
# Spawns real python3 subprocesses — requires python3 in $PATH.
# After the 2025-11 file split, the module is a directory with one submodule
# per concern; we sweep the whole tree.
mutants-python-repl:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/python_repl.rs" \
        --file "crates/omega-tools/src/python_repl/*.rs"

# Run cargo-mutants targeted at the shared process-kill helpers (kill_group, kill_soft).
# Both functions route through the shell; mutations that swap SIGKILL for SIGINT or
# alter the negated-pgid sign are caught by the timeout integration tests.
mutants-process-util:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/process_util.rs"

# Run cargo-mutants targeted at the external-editor prompt composition helpers
# (resolve_editor_command precedence OMEGA_EDITOR→VISUAL→EDITOR, split_editor_command).
# These back the browser "✎ Editor" button / POST /api/compose. The async I/O
# launcher compose_with_editor is #[mutants::skip] (process/tempfile edge); the
# pure helpers carry the budget, covered by editor.rs unit tests + the
# subprocess /api/compose integration tests. Template: mutants-process-util.
# Uses --in-place to avoid copying the large (incl. leptos wasm) target tree,
# and `-- --lib` to run only the fast editor.rs unit tests (the subprocess
# /api/compose integration tests exercise the skipped I/O launcher, not the
# mutated pure helpers, and would make each mutant run minutes long).
mutants-compose-editor:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --in-place --cap-lints=true --file "crates/omega-server/src/editor.rs" -- --lib

# Run cargo-mutants targeted at the async-monitor runtime (Monitors Phase 1).
# Mutates the MonitorManager (spawn / stop / shutdown / queue + roster
# mutations) and runs the omega-tools suite incl. the 9 monitor E2E tests.
# Spawns real bash subprocesses (printf / sleep / seq) — requires bash.
# Template: mutants-process-util (see AGENTS.md).
mutants-monitors:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/monitors.rs"

# Run cargo-mutants targeted at the two monitor tool wrappers (Monitors Phase 1):
# monitor() (spawn + MonitorStarted extra_event) and stop_monitor() (kill +
# MonitorStopped/StoppedByAgent extra_event, no-op on unknown/dead). Exercised
# via execute_tool in the monitor E2E tests. Template: mutants-process-util.
mutants-monitor-tools:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/tools/monitor.rs" \
        --file "crates/omega-tools/src/tools/stop_monitor.rs"

# Run cargo-mutants targeted at run_background.rs (background-job -> monitor folding).
# A background job IS a monitor whose command redirects output to a logFile, so it
# streams zero deliveries and emits exactly one MonitorStopped on exit. Covers the
# logFile path construction, the `exec > logFile 2>&1` wrapped command, cwd
# passthrough, the atomic seq counter (log-file uniqueness), the MonitorStarted
# extra_event, and the { id, logFile, pid } return. Exercised via execute_tool in
# the background-job E2E tests in monitors.rs. Template: mutants-process-util.
mutants-bg-run-background:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/tools/run_background.rs"

# Run cargo-mutants targeted at write_stdin.rs (manager-routed stdin writes).
# The handle is now an `id` (not a pid); write_stdin reaches both background jobs
# and streaming monitors. Covers id-arg parsing, write vs close (end_stdin)
# branching, and error propagation. Exercised via execute_tool against both a
# background job and a streaming monitor. Template: mutants-process-util.
mutants-bg-write-stdin:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/tools/write_stdin.rs"

# Run cargo-mutants targeted at the python3 bootstrap logic in python_repl.rs.
# Covers is_not_found(), start_inner() branching (AptNotFound / AptFailed /
# Succeeded), retry logic, and the BootstrapInfo return path.
# bootstrap_python3() and run_apt_get() are marked #[mutants::skip] because
# they call real OS processes; the logic branches they implement are exercised
# via mock-closure unit tests in start_inner.
mutants-python-repl-bootstrap:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/python_repl/bootstrap.rs"

# Run cargo-mutants targeted at the REPL resume guard in session_resume.rs.
# Verifies that the ReplResumeUnsupported check cannot be mutated away.
# Template: mutants-system-prompt-guard (see AGENTS.md).
mutants-repl-resume:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/session_resume.rs"

# Run cargo-mutants targeted at the Phase-6 unified bottom panels:
# - serialize/parse_panels_open (localStorage open-set persistence)
# - any_panel_activity (activity-dot derivation on the Panels button)
#
# Covers lib.rs (storage serialise/parse) and composer.rs (activity).
# Template: mutants-system-prompt-guard (see AGENTS.md).
mutants-bottom-panels:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place --cap-lints=true \
        --cargo-arg=--no-default-features --cargo-arg=--features=ssr \
        --file "src/lib.rs" \
        --file "src/composer.rs" \
        --file "src/monitors_panel.rs" \
        --file "src/queue_panel.rs"

# Run cargo-mutants targeted at the absolute sessions-root resolution used by
# GET /api/sessions, so the picker's "Copy @path" button yields an absolute
# reference. Scoped to `absolute_sessions_root` (relative-default anchoring,
# absolute-root passthrough) and `list_sessions` (per-item `path`). The
# integration assertion lives in tests/http.rs::get_sessions_item_path_is_absolute.
mutants-sessions-root:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --in-place --cap-lints=true \
        --file "crates/omega-server/src/router.rs" --re 'absolute_sessions_root|list_sessions'

# §15 U1 (Unified Input Model) — the persistent per-session agent loop.
# Scoped to Agent::run + Agent::drive_turn in agent.rs.  Both are `stream!`
# macro generators, so cargo-mutants can only mutate the outer fn body (the
# macro body is opaque); the body-replacement mutants come back UNVIABLE
# (`Default::default()` is not implemented for `Pin<Box<dyn Stream>>`).
# Uses --in-place to avoid copying the 6 GB target directory to TMPDIR.
mutants-agent-run-loop:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
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
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'drive_turn'

# §15 HarnessRecovery event type — new OmegaEvent::HarnessRecovery variant, the
# HarnessRecoveryKind enum, HarnessRecoveryEvent struct, and time() accessor.
# Mutates omega-types/src/events.rs and runs the omega-types test suite.
# All mutations must be CAUGHT or UNVIABLE.
mutants-harness-recovery-events:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-types --in-place --cap-lints=true --file "crates/omega-types/src/events.rs"

# §15 inject_harness_recovery helper — the free method on Agent that
# emits the HarnessRecovery event + appends to context/history.
# The call sites inside the async_stream! macro report UNVIABLE
# (stream generators are opaque to cargo-mutants); the helper itself,
# being a plain async method, should have its mutations CAUGHT.
# Uses --in-place to avoid copying the 6 GB target directory.
mutants-harness-recovery-agent:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
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
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place --file "crates/omega-agent/src/agent.rs" \
        --re 'inject_user_message|inject_dangling_tool_results|inject_tool_results_batch'

# §15 U1 — the server glue for the persistent run task: handle_user_message
# (now just inbox.send), spawn_run_task (owns the agent lock + forwards the
# run stream to WS, incl. turn-state + roster pushes), and teardown_prior_run
# (abort + run_cancel + join + session-end monitor reap).  Exercised by the
# two_sequential_user_messages_share_one_run_task and
# reset_reaps_prior_sessions_live_monitor tests in tests/ws_router.rs.
mutants-server-run-task:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --in-place --cap-lints=true \
        --file "crates/omega-server/src/router.rs" \
        --re 'handle_user_message|spawn_run_task|teardown_prior_run'

# Run cargo-mutants targeted at the picker's `session_at_path` formatter, which
# wraps the server-supplied absolute path as an `@<path>/` composer reference.
# Runs on the wasm target (the only one the leptos crate's tests build for).
mutants-session-at-path:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place \
        --cargo-arg=--target=wasm32-unknown-unknown --cap-lints=true \
        --file "src/picker.rs" --re 'session_at_path'

# Run cargo-mutants targeted at the context-modal `render_block` projection,
# which formats one content block (text / tool_use / tool_result / thinking)
# to its display string — including the inline tool-id labels that let a
# tool_use and its tool_result be paired by the protocol's opaque id.
# Runs on the wasm target (the only one the leptos crate's tests build for).
mutants-render-block:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place \
        --cargo-arg=--target=wasm32-unknown-unknown --cap-lints=true \
        --file "src/context_modal.rs" --re 'render_block'

# Run cargo-mutants targeted at the WS protocol projection (§16: the
# WS-message mirror elimination). Covers `WsEnvelope` (the thin
# frontend-only frame enum), the `#[serde(untagged)] WsMessage`
# { Envelope, Event(OmegaEvent) } routing, and the `From` impls. The
# drift-guard test round-trips every OmegaEvent variant through the
# server-serialize → WsMessage parse path, so a survivor here means a
# real coverage gap. Runs on the wasm target (the only one the leptos
# crate's tests build for).
mutants-ws-protocol:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place \
        --cargo-arg=--target=wasm32-unknown-unknown --cap-lints=true \
        --file "src/protocol.rs"

# Run cargo-mutants targeted at the schemas.rs tool-definition filtering.
# Covers the tool_definitions(tool_selection) membership-driven filtering,
# canonical-order iteration, and the shell-aware fetch_url schema branch.
mutants-schemas:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/schemas.rs"

# Run cargo-mutants targeted at the fetch_url tool implementation.
# Covers the shell-aware branch (driven by shell-tool presence in
# tool_selection), the postprocess path, and the apply_shell_gated_cap
# truncation logic.
# Requires network access (real HTTP fetches to example.com).
mutants-fetch-url:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/tools/fetch_url.rs"

# Run cargo-mutants targeted at the system_prompt.rs block assembly.
# Covers file-tool-absent and shell-tool-absent branches (driven by
# tool_selection membership), the python_repl addendum, and the combined
# reduced_toolset_addendum.
mutants-system-prompt:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/system_prompt.rs"

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
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true --file "crates/omega-tools/src/lib.rs"

# Phase 2.2.1 — full python_repl module sweep (includes repl.rs constant change).
# Covers MAX_TIMEOUT_SECS constant and repl.execute() defence-in-depth clamp.
mutants-python-repl-221:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/python_repl/repl.rs"

# Phase 2.2.1 — system_prompt.rs: timeout constants + sh() / SyntaxWarning.
# Alias for mutants-system-prompt scoped to the 2.2.1 additions.
mutants-system-prompt-221:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true --file "crates/omega-agent/src/system_prompt.rs"

# Nudging revamp — system_prompt.rs: monitor_addendum behavioral rules.
# Scoped to monitor_addendum: verifies that mutations weakening the
# not-the-user / don't-fabricate / end-turn-and-wait rules are caught by
# the monitor_addendum_contains_* tests.
mutants-monitor-addendum:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --in-place --cap-lints=true \
        --file "crates/omega-agent/src/system_prompt.rs" --re 'monitor_addendum'

# Phase 2.3 — event_view.rs: python_repl arm in tool_call_preview.
# Must be run from the frontends/leptos directory since omega-web is
# a standalone workspace.  All mutations must be caught or unviable.
mutants-python-repl-23:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place --cap-lints=true         --file "src/event_view.rs"

# Phase 3 (UI: roster badge + modal) — is_monitor_event and roster_snapshot_msg
# in router.rs.  These are the two non-trivial decision functions that govern
# (a) WHICH events trigger a follow-up roster push, and (b) HOW MonitorInfo is
# projected into the MonitorRosterItem wire format.  All mutations must be
# caught or unviable; the connect-time and per-event WS tests + router.rs unit
# tests provide coverage.
mutants-monitor-roster-push:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --in-place --cap-lints=true \
        --file "crates/omega-server/src/router.rs" --re 'is_monitor_event|roster_snapshot_msg'

# Phase 3 (UI: roster badge + modal) — MonitorRoster serialisation in
# ws_message.rs.  NOTE: cargo-mutants finds 0 mutants here because
# serde_json::json!{...} is a macro call, not a regular function body.
# Coverage is provided instead by the ws_message unit tests that snapshot
# the exact JSON output (type field, monitors array, every item field);
# any change to the wire format immediately breaks those tests.
# This recipe is kept as documentation of the decision.
mutants-monitor-ws-message:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --in-place --cap-lints=true \
        --file "crates/omega-server/src/ws_message.rs" --re 'MonitorRoster|monitor_roster'

# Phase 5 — monitors_panel.rs: badge_label + running_count + total_fired.
# These three pure derivation functions carry the mutation-test budget for the
# monitors panel (the view/component body itself is #[mutants::skip]).
# Tests: wasm-bindgen-test unit tests in monitors_panel.rs + snapshot tests.
mutants-monitors-panel:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place --cap-lints=true \
        --file "src/monitors_panel.rs" \
        --no-default-features --features ssr

# Phase 2.3 — feed.rs: ToolUseBlock dispatch (timeout chip + expansion body).
# Scoped to the ToolUseBlock-related logic via --regex to keep runtime
# manageable.  feed.rs is otherwise mostly JS-interop glue exempt from
# mutation testing (see component docs in feed.rs).
mutants-feed-tool-use-23:
    {{mutants-guard}}
    cd frontends/leptos && {{mutants-mem-cap}} cargo mutants --in-place --cap-lints=true         --file "src/feed.rs"         --regex "PYTHON_REPL_DEFAULT_TIMEOUT_SECS|timeout_chip|python_repl"

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
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place \
        --file "crates/omega-agent/src/input_queue.rs"

# §15 U1 — mutation-test the server push decision points in router.rs.
# Target: `is_user_message_event`, `queue_snapshot_msg`, enqueue/drain push hooks.
mutants-input-queue-router:
    {{mutants-prep}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --cap-lints=true \
        --file "crates/omega-server/src/router.rs" \
        -- --tmp-dir {{mutants-tmp}}

# §15 U2 — the MonitorSink routing in the MonitorManager: `push_stdout` /
# `enqueue_stopped` now route to the attached sink (the inbox) and fall back
# to the pending queue only when no sink is attached; `attach_sink` installs
# it.  Both branches are covered by omega-tools tests:
# `attached_sink_receives_stdout_and_stop_bypassing_pending_queue` (sink) and
# the existing no-sink stdout/stop tests (fallback).  §17 Phase A added
# `push_stderr` sink-routing (deliver_stderr the instant a line is read,
# else pending-queue fallback) — covered by
# `attached_sink_receives_stderr_bypassing_pending_queue` (sink) and the
# `stderr_lines_reach_queue_and_roster_tail` no-sink test (fallback).
# All mutations must be CAUGHT or UNVIABLE.
mutants-u2-monitor-sink:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/monitors.rs" \
        --re 'push_stdout|push_stderr|enqueue_stopped|attach_sink|MonitorManager::sink'

# §15 U2 — the InputQueue additions (drain_pending, on_change firing in push,
# make_view monitor-source arms, InboxSink).  Covered by the input_queue unit
# tests.  Reuses the U1 file scope; all mutations must be CAUGHT or UNVIABLE.
mutants-u2-input-queue: mutants-input-queue

# §15 U2 — the plain-async drain/routing helper on Agent:
# `inject_input_item` (routes each drained InputItem through its inject_*
# helper).  The seam drains themselves live inside the async_stream! macro
# body and come back UNVIABLE (opaque generators); the helper is CAUGHT by
# the Seam-A/Seam-B/batching tests in tests/internal.rs.
# (§17 Phase A removed the `drain_monitor_stderr` stderr drain entirely —
# monitor stderr now emits straight through the EventSink at production time;
# see `mutants-event-sink` and `mutants-u2-monitor-sink`.)
# Uses --in-place to avoid copying the 6 GB target dir.
mutants-u2-drain-routing:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place \
        --file "crates/omega-agent/src/agent.rs" \
        --re 'inject_input_item'

# §15 U3 — the Halt/Resume control-reconciliation state machine in controls.rs
# (request_halt / take_halt_request / request_resume / take_resume_request /
# enter_halt_wait / exit_halt_wait / request_abort).  These are the pure,
# synchronous control primitives the run loop calls at each seam; the actual
# park `tokio::select!` lives inside the async_stream! macro and is out of
# scope (opaque generator).  Covered by the controls.rs unit tests.
# All mutations must be CAUGHT or UNVIABLE — a survivor means a test gap.
mutants-u3-controls:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place \
        --file "crates/omega-agent/src/controls.rs"

# §15 U3 — the turn-state marker derivation + Halt/Resume WS handlers in
# router.rs: `next_turn_state_for` (event → idle/running/halted block-boundary
# projection), `handle_halt` (gate on running + request_halt), and
# `handle_resume` (gate on halted + request_resume).  Covered by the
# ws_router.rs Halt/Resume/dogfood WS tests + router unit tests.
# All mutations must be CAUGHT or UNVIABLE.
mutants-u3-turn-state:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --cap-lints=true --in-place \
        --file "crates/omega-server/src/router.rs" \
        --re 'next_turn_state_for|handle_halt|handle_resume'

# §17 Phase A — the EventSink: out-of-band append+broadcast from any caller.
# `event_sink.rs` is small and self-contained: EventSink::emit (append THEN
# broadcast), emit_detached (broadcast in-order THEN spawn append),
# set_broadcaster, store, and the EventBroadcaster trait.  CAUGHT by the
# controls.rs `request_halt_emits_once_disk_time_equals_wire_time` test
# (disk==wire timestamp, exactly-once) and the tests/internal.rs monitor-stderr
# and mid-turn-model-change tests (broadcast + disk content).  --in-place to
# avoid copying the 6 GB target dir.
mutants-event-sink:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place \
        --file "crates/omega-agent/src/event_sink.rs"

# §17 Phase A — the EventSink wiring on Agent: `ModelEffortHandle::set_model`
# and `::set_effort` (build the change event, emit it through the sink at
# click time, return it), `Agent::set_event_broadcaster` (installs the WS
# broadcaster into the sink), and `Agent::model_effort_handle` (hands out a
# handle over the SAME shared model/effort Arcs + sink).  The `drive_turn`
# entry snapshot lives inside the async_stream! macro body and comes back
# UNVIABLE (opaque generator).  CAUGHT by the tests/internal.rs
# `mid_turn_model_change_snapshots_and_applies_next_turn`,
# `active_model_reflects_set_model` and `active_effort_reflects_set_effort`
# tests.  --in-place to avoid copying the 6 GB target dir.
mutants-event-sink-wiring:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-agent --cap-lints=true --in-place \
        --file "crates/omega-agent/src/agent.rs" \
        --re 'ModelEffortHandle|set_event_broadcaster|model_effort_handle'

# §17 Phase A — the server's WS half of the EventSink: `WsEventBroadcaster`
# (resolves the CURRENT ws_tx cell at emit time and forwards the event as a
# WsMessage::Item), plus the `handle_set_model` / `handle_set_effort` router
# handlers that now route their change events through the sink (no direct
# tx.send).  CAUGHT end-to-end by the ws_router.rs `set_model_*` and Halt WS
# tests, which assert the client receives the frame — now delivered via the
# broadcaster.  --in-place to avoid copying the 6 GB target dir.  --timeout 45
# so a frame-suppressing mutant trips the WS tests' own 30 s recv timeout
# (→ CAUGHT) instead of cargo-mutants' 20 s auto-timeout (→ TIMEOUT).
mutants-ws-event-broadcaster:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-server --cap-lints=true --in-place --timeout 45 \
        --file "crates/omega-server/src/session.rs" \
        --file "crates/omega-server/src/router.rs" \
        --re 'WsEventBroadcaster|handle_set_model|handle_set_effort|set_ws_tx|send_via_ws_tx'

# Run cargo-mutants targeted at the redesigned file-editing tools:
# - text_match.rs: the opencode-style fuzzy-match cascade (the 9 replacers +
#   the replace() driver that resolves old_text → a single region or reports
#   NotFound/Ambiguous).
# - edit_file.rs: the flat single-edit tool (old_text→new_text, replace_all)
#   plus the shared summarize()/format_replace_error() helpers.
# - multi_edit_file.rs: sequential + atomic batch edits to one file.
# - format.rs: the edit_file/multi_edit_file arms of format_tool_call (the
#   human-readable log-line rendering of a tool call).
# All exercised via execute_tool in tests/file_tools.rs, plus the pure-function
# carve-out unit tests in text_match.rs and the format_tool_call unit tests.
# All mutations must be CAUGHT or UNVIABLE — no survivors. Template:
# mutants-system-prompt-guard (see AGENTS.md).
#
# Runs at the standard -j1 (see the Parallelism note above the `mutants`
# recipe): a wide sweep (4 files), but `-j` measurably does not help and the
# old -j1 disk-headroom pin happens to coincide with the measured optimum.
mutants-edit-tools:
    {{mutants-guard}}
    {{mutants-mem-cap}} cargo mutants -p omega-tools --in-place --cap-lints=true \
        --file "crates/omega-tools/src/tools/text_match.rs" \
        --file "crates/omega-tools/src/tools/edit_file.rs" \
        --file "crates/omega-tools/src/tools/multi_edit_file.rs" \
        --file "crates/omega-tools/src/format.rs"
