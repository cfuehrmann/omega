import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for Omega web UI end-to-end tests.
 *
 * Two test projects:
 *
 * 1. "chromium" — runs against the lightweight test-server (port 3001).
 *    The test-server speaks the same WebSocket protocol as the real server
 *    but bypasses the Agent entirely. Covers UI rendering, reconnect logic,
 *    history replay from injected events, etc.
 *
 * 2. "real-server" — runs against the real production omega-server (Rust,
 *    port 3003) wrapped by the mock-omega-server fixture binary, which
 *    injects a deterministic mock LLM provider and exposes a control HTTP
 *    API on port 3004 (/control/llm-calls, /control/reset-calls). Catches
 *    bugs in the production server code path (router, persistence,
 *    SIGTERM teardown, etc.) that the test-server cannot detect.
 *
 * Run: npx playwright test
 *   or: just e2e
 *
 * Requires a built frontend: just web-build (or just test-browser which builds first)
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,   // single test server instance
  retries: 0,
  workers: 1,
  reporter: "list",
  timeout: 15000,         // per-test cap; keeps cascading failures from dragging on
  globalTimeout: 300000,  // full-suite backstop: 5 minutes
  maxFailures: 1,         // stop after first failure — failures tend to cascade

  projects: [
    // -----------------------------------------------------------------------
    // Project 1: test-server (mock events, no Agent)
    // -----------------------------------------------------------------------
    {
      name: "chromium",
      testIgnore: [
        "**/real-server-replay.spec.ts",
        "**/pause-resume-interject.spec.ts",
        "**/web-ui-rename-session.spec.ts",
        "**/leptos-smoke.spec.ts",
        "**/leptos-session-picker.spec.ts",
        "**/leptos-conversation-feed.spec.ts",
      ],
      use: {
        ...devices["Desktop Chrome"],
        baseURL: "http://localhost:3001",
      },
    },

    // -----------------------------------------------------------------------
    // Project 2: real-server (real Agent + mock CreateMessageStream)
    // -----------------------------------------------------------------------
    {
      name: "real-server",
      testMatch: [
        "**/real-server-replay.spec.ts",
        "**/pause-resume-interject.spec.ts",
        "**/web-ui-rename-session.spec.ts",
        "**/leptos-smoke.spec.ts",
        "**/leptos-session-picker.spec.ts",
        "**/leptos-conversation-feed.spec.ts",
      ],
      use: {
        ...devices["Desktop Chrome"],
        baseURL: "http://localhost:3003",
      },
    },
  ],

  // Start both servers before running tests.
  // The real-server entry uses the Rust mock-omega-server fixture binary;
  // build it ahead of time with `just rust-build-mock-server` (recipe
  // `test-browser` and friends do this automatically).
  webServer: [
    {
      command: "bun run e2e/fixtures/test-server.ts",
      port: 3001,
      reuseExistingServer: false,
      timeout: 15000,
      gracefulShutdown: { signal: "SIGTERM", timeout: 5000 },
    },
    {
      command:
        "OMEGA_ALLOW_DIRTY=1 rust/target/release/mock-omega-server --port 3003 --ctrl-port 3004 --public-dir src/web/public --sessions-root .omega/test-sessions --leptos-dir frontends/leptos/dist",
      port: 3003,
      reuseExistingServer: false,
      timeout: 15000,
      gracefulShutdown: { signal: "SIGTERM", timeout: 5000 },
    },
  ],
});
