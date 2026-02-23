# Omega — common workflows
# Run `just --list` to see all recipes.

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
web-build:
    cd src/web && npx vite build

# Start the web server (serves built client on :3000)
web:
    bun run src/web/server.ts

# Start Vite dev server for web client (:5173, proxies WS to :3000)
# Run `just web` in another terminal first.
web-dev:
    cd src/web && npx vite

# Log in / refresh OAuth token
login:
    bun run src/login.ts

# Push current branch to origin
push:
    git push

# Run a quick git status
status:
    git status --short
