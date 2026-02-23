import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for Omega web UI end-to-end tests.
 *
 * The test server (e2e/fixtures/test-server.ts) is started automatically
 * by Playwright as a Bun subprocess. It speaks the same WebSocket protocol
 * as the real Bun server (src/web/server.ts) but without real Anthropic auth.
 *
 * Run: npx playwright test
 *   or: just e2e
 *
 * Requires a built frontend: just build (or just e2e which builds first)
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,   // single test server instance
  retries: 0,
  workers: 1,
  reporter: "list",

  use: {
    baseURL: "http://localhost:3001",
    trace: "on-first-retry",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  // Start the Bun test server before running tests
  webServer: {
    command: "bun run e2e/fixtures/test-server.ts",
    port: 3001,
    reuseExistingServer: false,
    timeout: 15000,
  },
});
