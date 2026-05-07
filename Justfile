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
#   just server         — starts omega-server on :3000 (Leptos at /)

# -----------------------------------------------------------------------
# Top-level test pipeline
# -----------------------------------------------------------------------

# Full quality gate: rust-gate + chromiumoxide e2e. Run before every commit.
# All output is tee'd to test-output/gate-latest.log (overwritten each run).
# On failure, read that file for the complete trace — no need to re-run.
gate:
    #!/usr/bin/env bash
    set -eo pipefail
    mkdir -p test-output
    LOG="test-output/gate-latest.log"
    BEFORE=$(ls -1 .omega/sessions/ 2>/dev/null | wc -l)
    {
        echo "=== rust-gate ==="
        just rust-gate
        echo "=== rust-e2e ==="
        just rust-e2e
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
    } 2>&1 | tee "$LOG"

# Run the chromiumoxide-driven Rust e2e suite. Builds the Leptos bundle
# and the mock-omega-server fixture binary first, then runs the
# `--ignored` (browser) tests in `omega-e2e`.
rust-e2e: web-leptos-build
    cd rust && cargo build --release -p omega-mock-server
    cd rust && cargo test -p omega-e2e --tests -- --ignored --test-threads=1

# -----------------------------------------------------------------------
# Leptos frontend
# -----------------------------------------------------------------------

# Build the Leptos frontend (trunk → frontends/leptos/dist/).
# Phase 3.7 made this the canonical production bundle; Phase 4 Q7
# flipped Trunk's `public_url` to `/` and omega-server now serves it
# from `/` (the `/leptos/` alias mount is gone).
web-leptos-build:
    rustup target add wasm32-unknown-unknown
    cd frontends/leptos && trunk build --release

# Run the Leptos crate's wasm-bindgen-test suite.
# `--lib` is required because the crate is lib + bin (Phase 3.6 split).
# The host-target snapshot harness lives at `tests/snapshots.rs` and
# is gated by `#[cfg(feature = "ssr")]` so it skips here.
web-leptos-test:
    rustup target add wasm32-unknown-unknown
    cargo install --locked --version =0.2.120 wasm-bindgen-cli
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

# Start the web server (serves the Leptos bundle + WebSocket on :3000).
# Pass any omega-server CLI args, e.g. just server --port 3001
server *args: rust-build-server web-leptos-build
    rust/target/release/omega-server {{args}}

# Show what's listening on :3000.
ports:
    @echo "=== :3000 (omega-server) ===" && lsof -iTCP:3000 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"

# -----------------------------------------------------------------------
# Rust quality gate
# -----------------------------------------------------------------------

# Rust-only gate: format check + Clippy + cargo test + cargo machete
# + Leptos wasm-bindgen-test suite + Leptos snapshot suite. Runs via
# the pre-commit hook when only rust/ files are staged.
# Run manually: just rust-gate
rust-gate: web-leptos-build web-leptos-test web-leptos-snapshots
    cd rust && cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo machete

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
    #!/usr/bin/env bash
    set -eo pipefail
    mkdir -p "$HOME/.cache/cargo-mutants-tmp"
    export TMPDIR="$HOME/.cache/cargo-mutants-tmp"
    cd rust && cargo mutants -j2

# Run cargo-mutants on the leptos crate (wasm32 target).
web-mutants:
    #!/usr/bin/env bash
    set -eo pipefail
    mkdir -p "$HOME/.cache/cargo-mutants-tmp"
    export TMPDIR="$HOME/.cache/cargo-mutants-tmp"
    rustup target add wasm32-unknown-unknown
    cargo install --locked --version =0.2.120 wasm-bindgen-cli
    cd frontends/leptos && cargo mutants -j2 --cargo-arg=--target=wasm32-unknown-unknown

# -----------------------------------------------------------------------
# Repo housekeeping
# -----------------------------------------------------------------------

# Push current branch to origin
push:
    git push

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

# Run a quick git status
status:
    git status --short
