# Omega — common workflows
# Run `just --list` to see all recipes.
#
# Two-terminal dev workflow:
#   Terminal 1:  just server   — Bun backend on :3000 (agent + WebSocket)
#   Terminal 2:  just client   — Vite hot-reload on :5173 (proxies WS → :3000)
#   Browser:     http://localhost:5173   (dev)  or  http://localhost:3000  (prod)
#
# Production (no hot-reload):
#   just build && just server

# Start Omega (terminal UI)
start:
    bun run src/ui-raw.ts

# Run all tests
test:
    bun test

# Run tests in watch mode
test-watch:
    bun test --watch

# Type-check + unused-exports audit
check:
    bun test && bunx knip

# Build the web client (Vite → src/web/public/)
build:
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

# Run a quick git status
status:
    git status --short
