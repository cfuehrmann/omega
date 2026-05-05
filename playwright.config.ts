import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for Omega web UI end-to-end tests.
 *
 * Phase 3.7 cutover: there is now one Playwright project, "real-server",
 * which runs against the real production omega-server (Rust, port 3003)
 * wrapped by the mock-omega-server fixture binary. The fixture injects
 * a deterministic mock LLM provider and exposes a control HTTP API on
 * port 3004 (`/control/llm-calls`, `/control/reset-calls`).
 *
 * The previous "chromium" project (Bun test-server.ts at :3001 driving
 * the SolidJS bundle) was retired alongside the SolidJS frontend; its
 * Leptos-snapshot-covered coverage is now in `tests/snapshots.rs`
 * (TEST-ARCH-5) and the surviving live-browser specs live under
 * `e2e/leptos-*.spec.ts`. Anything that needed reconnect / replay /
 * pause-during-stream / real-server-side-effect coverage ports to
 * Phase 4 (chromiumoxide + LLM oracle).
 *
 * Run: npx playwright test
 *   or: just e2e
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
    {
      name: "real-server",
      use: {
        ...devices["Desktop Chrome"],
        baseURL: "http://localhost:3003",
      },
    },
  ],

  webServer: [
    {
      command:
        "OMEGA_ALLOW_DIRTY=1 rust/target/release/mock-omega-server --port 3003 --ctrl-port 3004 --sessions-root .omega/test-sessions --leptos-dir frontends/leptos/dist",
      port: 3003,
      reuseExistingServer: false,
      timeout: 15000,
      gracefulShutdown: { signal: "SIGTERM", timeout: 5000 },
    },
  ],
});
