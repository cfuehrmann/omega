/**
 * Claude Max OAuth dialog — end-to-end tests.
 *
 * Verifies that:
 *   1. Selecting "Claude Max" in the auth dropdown always opens the dialog
 *      (regardless of any previously stored token).
 *   2. The dialog stays open showing "Generating authorization link…" until
 *      the server pushes an oauth_url event.
 *   3. Once the oauth_url arrives, the clickable link is shown.
 *   4. The user can paste a code and submit — dialog closes and the correct
 *      message is sent to the server.
 *   5. The user can cancel — dialog closes and cancel_oauth is sent.
 *   6. Re-selecting "Claude Max" while already in claude-max mode is not
 *      tested here because the browser's change event does not fire when the
 *      already-selected option is chosen.  The real robustness guarantee is
 *      in the server: requestClaudeMaxSwitch() always starts a fresh OAuth
 *      flow and never returns "switched".
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Seed session_info + auth mode so the session bar and auth dropdown become visible. */
async function seedApiKeyMode(server: import("./fixtures/index.js").ServerHelper) {
  await server.sendEvent({ type: "session_info", dir: "/tmp/test-session" });
  await server.sendEvent({ type: "auth", mode: "api-key" });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test("selecting Claude Max opens the OAuth dialog immediately", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  // Wait for dropdown to appear
  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });

  // Choose Claude Max
  await select.selectOption("claude-max");

  // Dialog should appear
  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });
});

test("dialog shows loading state before oauth_url arrives", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  // While no oauth_url has been sent, show the loading placeholder
  await expect(page.locator(".oauth-loading")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".oauth-loading")).toHaveText("Generating authorization link…");
});

test("selecting Claude Max sends set_auth_mode to server", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  const msg = await server.nextMessage() as any;
  expect(msg.type).toBe("set_auth_mode");
  expect(msg.mode).toBe("claude-max");
});

test("oauth_url event shows the authorization link in the dialog", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  // Dialog must be open first
  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  // Server pushes the authorization URL
  const authUrl = "https://claude.ai/oauth/authorize?code=true&client_id=test";
  await server.sendEvent({ type: "oauth_url", url: authUrl });

  // Link appears; loading placeholder disappears
  const link = page.locator(".oauth-link");
  await expect(link).toBeVisible({ timeout: 3000 });
  await expect(link).toHaveText("Open authorization page ↗");
  await expect(link).toHaveAttribute("href", authUrl);
  await expect(page.locator(".oauth-loading")).not.toBeVisible();
});

test("submitting a code sends submit_oauth_code and closes the dialog", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  // Drain the set_auth_mode message
  await server.nextMessage();

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  // Send oauth_url so the submit button is reachable (it's always enabled once code is typed)
  await server.sendEvent({ type: "oauth_url", url: "https://claude.ai/oauth/authorize?test=1" });
  await expect(page.locator(".oauth-link")).toBeVisible({ timeout: 3000 });

  // Type a code
  const input = page.locator(".oauth-code-input");
  await input.fill("abc123#state456");

  // Submit
  await page.locator(".oauth-submit-btn").click();

  // Correct message sent
  const msg = await server.nextMessage() as any;
  expect(msg.type).toBe("submit_oauth_code");
  expect(msg.code).toBe("abc123#state456");

  // Dialog is closed
  await expect(page.locator(".oauth-overlay")).not.toBeVisible({ timeout: 3000 });
});

test("Enter key submits the code", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  await server.nextMessage(); // drain set_auth_mode

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });
  await server.sendEvent({ type: "oauth_url", url: "https://claude.ai/oauth/authorize?test=1" });
  await expect(page.locator(".oauth-link")).toBeVisible({ timeout: 3000 });

  const input = page.locator(".oauth-code-input");
  await input.fill("mycode#mystate");
  await input.press("Enter");

  const msg = await server.nextMessage() as any;
  expect(msg.type).toBe("submit_oauth_code");
  expect(msg.code).toBe("mycode#mystate");

  await expect(page.locator(".oauth-overlay")).not.toBeVisible({ timeout: 3000 });
});

test("cancelling the dialog sends cancel_oauth and closes the dialog", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  await server.nextMessage(); // drain set_auth_mode

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  await page.locator(".oauth-cancel-btn").click();

  const msg = await server.nextMessage() as any;
  expect(msg.type).toBe("cancel_oauth");

  await expect(page.locator(".oauth-overlay")).not.toBeVisible({ timeout: 3000 });
});

test("Escape key cancels the dialog", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  await server.nextMessage(); // drain set_auth_mode

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });
  await server.sendEvent({ type: "oauth_url", url: "https://claude.ai/oauth/authorize?test=1" });
  await expect(page.locator(".oauth-link")).toBeVisible({ timeout: 3000 });

  // Press Escape while focus is in the input
  const input = page.locator(".oauth-code-input");
  await input.focus();
  await input.press("Escape");

  const msg = await server.nextMessage() as any;
  expect(msg.type).toBe("cancel_oauth");

  await expect(page.locator(".oauth-overlay")).not.toBeVisible({ timeout: 3000 });
});

test("dialog stays open if auth_mode_changed arrives from a prior cached session", async ({ page, server }) => {
  // This is the regression test for the bug where a cached token caused the
  // server to emit auth_mode_changed immediately, closing the dialog before
  // the user could authenticate.
  //
  // The fix: requestClaudeMaxSwitch() never returns "switched" — it always
  // starts a fresh OAuth flow.  This test simulates the worst case by pushing
  // auth_mode_changed directly; the dialog must stay open because
  // claudeMaxDialogOpen was set true by the user's click, and it is only
  // cleared by the user submitting or cancelling — not by auth_mode_changed.

  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  // Dialog must be visible
  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  // Even if auth_mode_changed arrives (stale cached-token behaviour), the
  // dialog must NOT close because claudeMaxDialogOpen is still true.
  await server.sendEvent({ type: "auth_mode_changed", authMode: "claude-max" });

  // Give the UI a moment to react
  await page.waitForTimeout(300);

  // Dialog still visible — user must be able to finish the OAuth flow
  await expect(page.locator(".oauth-overlay")).toBeVisible();
});
