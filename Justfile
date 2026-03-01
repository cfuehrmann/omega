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

# Start Omega (terminal UI)
start:
    bun run src/ui-raw.ts

# Run all tests in parallel: web-build first, then test-core and test-browser concurrently.
# Outputs are captured and printed sequentially (core first, then browser) for readability.
test:
    #!/usr/bin/env bash
    set -euo pipefail
    cd src/web && npx vite build
    cd ../..
    CORE_OUT=$(mktemp); BROWSER_OUT=$(mktemp)
    bun test >"$CORE_OUT" 2>&1 & CORE_PID=$!
    npx playwright test >"$BROWSER_OUT" 2>&1; BROWSER_EXIT=$?
    wait $CORE_PID; CORE_EXIT=$?
    cat "$CORE_OUT"; cat "$BROWSER_OUT"
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

# Run browser (Playwright) tests — builds frontend first
test-browser: web-build
    npx playwright test

# Run browser tests with headed browser (useful for debugging)
test-browser-debug: web-build
    npx playwright test --headed

# Operator-run gate: knip + full test suite (test-core and test-browser run in parallel).
# Never invoked automatically by Omega — operator-only.
gate: test
    bunx knip

# Build the web client (Vite → src/web/public/)
web-build:
    cd src/web && npx vite build

# Start the web server (serves built client + WebSocket on :3000)
server:
    bun run src/web/server.ts

# Start Vite dev server for web client (:5173, proxies WS to :3000)
# Run `just server` in another terminal first.
client:
    cd src/web && npx vite

# Show what's listening on :3000 and :5173
ports:
    @echo "=== :3000 (Bun server) ===" && lsof -iTCP:3000 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"
    @echo "=== :5173 (Vite client) ===" && lsof -iTCP:5173 -sTCP:LISTEN -P -n 2>/dev/null || echo "  nothing"

# Log in / refresh OAuth token
login:
    bun run src/login.ts

# Push current branch to origin
push:
    git push

# Install git hooks (pre-commit test gate)
install-hooks:
    cp scripts/pre-commit .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    @echo "✅  Git hooks installed."

# Run a quick git status
status:
    git status --short
