# Omega — common workflows
# Run `just --list` to see all recipes.
#
# After Phase 4 there is one production frontend (Leptos, wasm32 via
# Trunk) and one Rust backend (omega-server). The Phase-4 Q7 deletion
# removed the entire TS toolchain (bun, vite, knip, tsconfigs,
# package.json, node_modules, Playwright) — the surviving e2e harness
# is `omega-e2e` (chromiumoxide-driven) and lives at
# rust/crates/omega-e2e.
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

# Format check + Clippy + cargo test + machete. Assumes dist/ is already built.
[private]
_rust-checks:
    cd rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
    cd frontends/leptos && cargo fmt --check
    cargo machete

# Build mock server + run browser tests. Assumes dist/ is already built.
[private]
_rust-e2e-run:
    cd rust && cargo build --release -p omega-mock-server
    cd rust && cargo test -p omega-e2e --tests -- --ignored --test-threads=1

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

# Build the production omega-server (release) — rust/target/release/omega-server
rust-build-server:
    cd rust && cargo build --release -p omega-server

# Build and start the web server (serves the Leptos bundle + WebSocket on :3000).
# Rebuilds the server binary and the Leptos bundle on every invocation.
# Pass any omega-server CLI args, e.g. just server --port 3001
server *args: rust-build-server web-leptos-build
    rust/target/release/omega-server {{args}}

# Show what's listening on :3000.
ports:
    @echo "=== :3000 (omega-server) ===" && lsof -iTCP:3000 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"

# -----------------------------------------------------------------------
# Rust quality gate
# -----------------------------------------------------------------------

# Auto-format all Rust workspaces (rust/ and frontends/leptos/).
# Run this manually any time; the pre-commit hook calls it automatically.
fmt:
    cd rust && cargo fmt
    cd frontends/leptos && cargo fmt
    @echo "✅  All Rust code formatted."

# Rust-only gate: format check + Clippy + cargo test + cargo machete
# + Leptos wasm-bindgen-test suite + Leptos snapshot suite. Runs via
# the pre-commit hook when only rust/ files are staged.
# Run manually: just rust-gate
#
# cargo machete is run from the repo root so it scans *both* the
# rust/ workspace and frontends/leptos/ in one pass. Running it from
# inside rust/ would silently skip the leptos workspace.
rust-gate: web-leptos-build web-leptos-test web-leptos-snapshots _rust-checks

# -----------------------------------------------------------------------
# Mutation testing
# -----------------------------------------------------------------------
#
# `cargo mutants` defaults to `/tmp` for per-mutant scratch trees. On this
# host `/tmp` is tmpfs (≈8 GB) which fills before the sweep finishes;
# redirect to `~/.cache/cargo-mutants-tmp` (real disk). Run sweeps with
# `-j2` to keep peak disk footprint reasonable.

# Run cargo-mutants on the rust workspace.
mutants:
    mkdir -p {{mutants-tmp}}
    cd rust && TMPDIR={{mutants-tmp}} cargo mutants -j2

# Run cargo-mutants on the leptos crate (wasm32 target).
web-mutants: wasm-setup
    mkdir -p {{mutants-tmp}}
    cd frontends/leptos && TMPDIR={{mutants-tmp}} cargo mutants -j2 --cargo-arg=--target=wasm32-unknown-unknown

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
