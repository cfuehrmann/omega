# Omega — common workflows
# Run `just --list` to see all recipes.
#
# After Phase 3.7 there is one production frontend (Leptos, wasm32 via Trunk)
# and one Rust backend (omega-server). The TS toolchain (bun, vite, knip,
# tsconfigs, package.json, node_modules) is retained only to run the
# surviving Playwright suite; it carries no application code. It exits
# alongside Playwright in Phase 4 (chromiumoxide + LLM oracle).
#
# Production:
#   just server         — starts omega-server on :3000 (Leptos at /)

# -----------------------------------------------------------------------
# Top-level test pipeline
# -----------------------------------------------------------------------

# Full quality gate: rust-gate + Playwright. Run before every commit.
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
        echo "=== test-browser ==="
        just test-browser
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

# Type-check what TypeScript still ships: only the e2e Playwright specs.
# (Phase 3.7 deleted `src/`, so the root + src/web/client tsconfigs are
# gone.)
typecheck:
    bunx tsgo -p e2e/tsconfig.json --noEmit

# Run browser (Playwright) tests — builds the Leptos bundle + the mock
# omega-server fixture binary first.
test-browser: web-leptos-build rust-build-mock-server
    npx playwright test

# Run browser tests with headed browser (useful for debugging).
test-browser-debug: web-leptos-build rust-build-mock-server
    npx playwright test --headed

# Run browser tests, saving full verbose output to a timestamped log file.
# Use this when debugging failures: the log path is printed, then inspect
# with read_file / grep_files. Never re-run just to see more output.
test-browser-log: web-leptos-build rust-build-mock-server
    #!/usr/bin/env bash
    LOG="test-output/playwright-$(date +%Y%m%d-%H%M%S).log"
    mkdir -p test-output
    npx playwright test --reporter=list > "$LOG" 2>&1; EC=$?
    echo "Log saved: $LOG"
    exit $EC

# Run targeted Playwright tests without rebuilding the Leptos bundle.
# Use when the build is already current.
# Examples:
#   just e2e e2e/leptos-markdown.spec.ts
#   just e2e --grep "reconnect"
e2e *args:
    npx playwright test {{args}}

# -----------------------------------------------------------------------
# Leptos frontend
# -----------------------------------------------------------------------

# Build the Leptos frontend (trunk → frontends/leptos/dist/).
# Phase 3.7 made this the canonical production bundle: omega-server
# now serves it from `/`. The `/leptos/` mount is retained for one
# release as an alias and will be removed in a follow-up PR.
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

# Build the mock omega-server used by Playwright real-server tests.
# Release binary at rust/target/release/mock-omega-server.
rust-build-mock-server:
    cd rust && cargo build --release -p omega-mock-server

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
