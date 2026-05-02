# Omega — common workflows
# Run `just --list` to see all recipes.
#
# Two-terminal dev workflow:
#   Terminal 1:  just server   — Bun backend on :3000 (agent + WebSocket)
#   Terminal 2:  just client   — Vite hot-reload on :5173 (proxies WS → :3000)
#   Browser:     http://localhost:5173   (dev)  or  http://localhost:3000  (prod)
#
# Production (no hot-reload):
#   just web-build && just server

# Run all tests in parallel: web-build first, then test-core and test-browser concurrently.
# Outputs are captured and printed sequentially (core first, then browser) for readability.
# set -e is intentionally absent: exit codes are captured explicitly so the cat calls
# always run and gate-latest.log always contains the full test output on failure.
test:
    #!/usr/bin/env bash
    set -uo pipefail
    cd src/web && npx vite build || exit $?
    cd ../..
    (cd rust && cargo build --release -p omega-mock-server) || exit $?
    CORE_OUT=$(mktemp); BROWSER_OUT=$(mktemp)
    bun test >"$CORE_OUT" 2>&1 & CORE_PID=$!
    npx playwright test >"$BROWSER_OUT" 2>&1 && BROWSER_EXIT=0 || BROWSER_EXIT=$?
    wait $CORE_PID && CORE_EXIT=0 || CORE_EXIT=$?
    echo "=== bun test ==="
    cat "$CORE_OUT"
    echo "=== playwright ==="
    cat "$BROWSER_OUT"
    rm -f "$CORE_OUT" "$BROWSER_OUT"
    exit $(( CORE_EXIT || BROWSER_EXIT ))

# Run bun tests (fast, no build needed)
test-core:
    bun test

# Run bun tests, stopping on first failure — fast feedback during iteration
# Prefer this (or a targeted single-file run: bun test src/foo.test.ts) over
# the full suite while working on a specific area.
test-fast:
    bun test --bail

# Run bun tests in watch mode
test-core-watch:
    bun test --watch

# Run browser (Playwright) tests — builds frontend + Rust binaries first
test-browser: web-build rust-build-mock-server
    npx playwright test

# Run browser tests with headed browser (useful for debugging)
test-browser-debug: web-build rust-build-mock-server
    npx playwright test --headed

# Run browser tests, saving full verbose output to a timestamped log file.
# Use this when debugging failures: the log path is printed, then inspect
# with read_file / grep_files. Never re-run just to see more output.
test-browser-log: web-build rust-build-mock-server
    #!/usr/bin/env bash
    LOG="test-output/playwright-$(date +%Y%m%d-%H%M%S).log"
    mkdir -p test-output
    npx playwright test --reporter=list > "$LOG" 2>&1; EC=$?
    echo "Log saved: $LOG"
    exit $EC

# Run targeted Playwright tests without rebuilding the web client.
# Use when the build is already current (e.g. you ran just web-build recently).
# Accepts any Playwright CLI arguments: file paths, --grep patterns, etc.
# Examples:
#   just e2e e2e/web-ui-mermaid.spec.ts
#   just e2e --grep "reconnect"
e2e *args:
    npx playwright test {{args}}

# Type-check all TypeScript (three passes in parallel: backend/tests, web client, e2e).
typecheck:
    #!/usr/bin/env bash
    set -euo pipefail
    OUT1=$(mktemp); OUT2=$(mktemp); OUT3=$(mktemp)
    bunx tsgo --noEmit >"$OUT1" 2>&1 & PID1=$!
    bunx tsgo -p src/web/client/tsconfig.json --noEmit >"$OUT2" 2>&1 & PID2=$!
    bunx tsgo -p e2e/tsconfig.json --noEmit >"$OUT3" 2>&1 & PID3=$!
    EC=0
    wait $PID1 || { cat "$OUT1"; EC=1; }
    wait $PID2 || { cat "$OUT2"; EC=1; }
    wait $PID3 || { cat "$OUT3"; EC=1; }
    rm -f "$OUT1" "$OUT2" "$OUT3"
    exit $EC

# Full quality gate: typecheck + full test suite + knip. Run before every commit.
# All output is tee'd to test-output/gate-latest.log (overwritten each run).
# On failure, read that file for the complete trace — no need to re-run.
# Works identically whether called directly or via the git pre-commit hook.
gate:
    #!/usr/bin/env bash
    set -eo pipefail
    mkdir -p test-output
    LOG="test-output/gate-latest.log"
    # Snapshot production session count before tests run
    BEFORE=$(ls -1 .omega/sessions/ 2>/dev/null | wc -l)
    {
        echo "=== typecheck ==="
        just typecheck
        echo "=== test ==="
        just test
        echo "=== knip ==="
        bunx knip
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

# Build the web client (Vite → src/web/public/)
web-build:
    cd src/web && npx vite build

# Build the production omega-server (Rust) — release binary at rust/target/release/omega-server
rust-build-server:
    cd rust && cargo build --release -p omega-server

# Build the mock omega-server used by Playwright real-server tests.
# Release binary at rust/target/release/mock-omega-server.
rust-build-mock-server:
    cd rust && cargo build --release -p omega-mock-server

# Start the web server (serves built client + WebSocket on :3000).
# Pass any omega-server CLI args, e.g. just server --port 3001
server *args: rust-build-server
    rust/target/release/omega-server {{args}}

# Start Vite dev server for web client (:5173, proxies WS to :3000)
# Run `just server` in another terminal first.
client:
    cd src/web && npx vite

# Show what's listening on :3000 and :5173
ports:
    @echo "=== :3000 (Bun server) ===" && lsof -iTCP:3000 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"
    @echo "=== :5173 (Vite client) ===" && lsof -iTCP:5173 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"

# Push current branch to origin
push:
    git push

# Tag the current commit with the version declared in omega_agent.py and push
# both the tag and the current branch to origin.
# Usage: just release
# The tag name is read automatically from OMEGA_VERSION in omega_agent.py so
# it is always in sync with what the benchmark containers will clone.
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

# Generate TypeScript bindings from Rust types via ts-rs.
# Exports all types annotated with #[ts(export)] to rust/bindings/.
# Re-run whenever Rust types change; then commit the updated .ts files.
rust-bindings:
    #!/usr/bin/env bash
    set -euo pipefail
    EXPORT_DIR="$(pwd)/rust/bindings"
    mkdir -p "$EXPORT_DIR"
    export TS_RS_EXPORT_DIR="$EXPORT_DIR"
    export TS_RS_LARGE_INT="number"
    cd rust
    cargo test -p omega-protocol --features ts-bindings -- export_bindings
    cargo test -p omega-core     --features ts-bindings -- export_bindings
    cargo test -p omega-store    --features ts-bindings -- export_bindings

# Rust-only quality gate: format check + Clippy + cargo test + bindings drift.
# Runs via the pre-commit hook when only rust/ files are staged.
# Run manually: just rust-gate
rust-gate:
    cd rust && cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo machete
    just rust-bindings
    @echo "=== bindings drift check ==="
    git diff --exit-code rust/bindings/ || (echo "\n❌  rust/bindings/ is out of date — run \`just rust-bindings\` and commit."; exit 1)

# Install git hooks (pre-commit test gate)
install-hooks:
    cp scripts/pre-commit .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    @echo "✅  Git hooks installed."

# Run a quick git status
status:
    git status --short
