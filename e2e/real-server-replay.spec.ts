/**
 * Session replay test against the real production server (src/web/server.ts).
 *
 * Uses the real-server fixture (e2e/fixtures/real-server.ts) which starts
 * runWebApp() with a mock StreamProvider — no real Anthropic API calls are made.
 *
 * Purpose: catch bugs in the production server code path that the test-server
 * (e2e/fixtures/test-server.ts) cannot detect, because the test-server never
 * calls the Agent constructor or writes events through the Agent.
 *
 * Regression: the Agent constructor was called with wrong argument positions in
 * server.ts, causing events to be written to the default fallback path instead
 * of the session-specific events.jsonl. Page reload found an empty file and
 * replayed nothing. This test would have caught that bug.
 */

import { test, expect } from "@playwright/test";

// Shorthand for waiting until the Ω button is in "connected" state.
const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

test("events persist through the real server and replay after page reload", async ({ page }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Send a message through the real agent (uses the mock provider → "pong")
  const textarea = page.locator("textarea");
  await textarea.fill("ping");
  await textarea.press("Enter");

  // Wait for the user block to appear
  await expect(page.getByTestId("block-user")).toBeVisible({ timeout: 5000 });

  // Wait for turn_end — the footer block signals the turn is complete
  await expect(page.getByTestId("block-turn-end")).toBeVisible({ timeout: 10000 });

  // Reload the page — simulates a browser refresh
  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  // The user message and LLM response must still be visible — replayed from disk
  await expect(page.getByTestId("block-user")).toBeVisible({ timeout: 5000 });
  await expect(page.getByTestId("block-turn-end")).toBeVisible({ timeout: 5000 });

  // The replayed assistant response should contain "pong"
  const llmResponseBlock = page.getByTestId("block-llm-response");
  await expect(llmResponseBlock).toBeVisible({ timeout: 5000 });
  await expect(llmResponseBlock).toContainText("pong");
});

// ---------------------------------------------------------------------------
// Abort during tool execution — subprocess must be killed promptly
// ---------------------------------------------------------------------------

test("abort during run_command kills subprocess and shows '⊘ Aborted' quickly", async ({ page }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Send the trigger message — the mock provider returns run_command(sleep 10)
  await page.locator("textarea").fill("abort_sleep_test");
  await page.locator("textarea").press("Enter");

  // Wait for the tool_call block — this confirms the sleep subprocess is running
  await expect(page.getByTestId("block-tool")).toBeVisible({ timeout: 10000 });

  // Click Abort — with the fix, the subprocess is killed immediately
  await page.getByRole("button", { name: "Abort" }).click();

  // '⊘ Aborted' must appear within 3 seconds.
  // Without the subprocess-kill fix this times out because sleep 10 keeps running.
  await expect(page.getByTestId("block-interrupt")).toContainText("Aborted", { timeout: 3000 });
});

test("session dir shown in bottom panel persists after reload", async ({ page }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Open the bottom panel (Ω button) to reveal session info
  await page.getByTestId("omega-btn").click();
  const sessionPanel = page.getByTestId("session-panel");
  await expect(sessionPanel).toBeVisible({ timeout: 3000 });
  const dirBefore = await page.getByTestId("session-dir").textContent();
  expect(dirBefore).toBeTruthy();

  // Send a message and wait for completion — use .first() in case a previous
  // test left a footer block on screen (real server is not reset between tests)
  await page.locator("textarea").fill("ping");
  await page.locator("textarea").press("Enter");
  await expect(page.getByTestId("block-turn-end").first()).toBeVisible({ timeout: 10000 });

  // Reload — same session should be maintained (same dir)
  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Re-open the panel after reload (panel state is not persisted)
  await page.getByTestId("omega-btn").click();
  await page.getByTestId("session-panel").waitFor({ timeout: 3000 });
  const dirAfter = await page.getByTestId("session-dir").textContent();
  expect(dirAfter).toBe(dirBefore);
});
