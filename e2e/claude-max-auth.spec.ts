/**
 * Claude Max OAuth dialog — end-to-end tests.
 *
 * Verifies that:
 *   1. Selecting "Claude Max" in the auth dropdown always opens the dialog.
 *   2. The dialog shows "Generating authorization link…" until oauth_url arrives.
 *   3. Once oauth_url arrives, the clickable link and "Waiting…" state are shown.
 *   4. The dialog closes automatically when the server sends auth_mode_changed
 *      (i.e. the localhost callback was received and exchange succeeded).
 *   5. The user can cancel — dialog closes and cancel_oauth is sent.
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

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });

  await select.selectOption("claude-max");

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });
});

test("dialog shows loading state before oauth_url arrives", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

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

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  const authUrl = "https://claude.ai/oauth/authorize?code=true&client_id=test";
  await server.sendEvent({ type: "oauth_url", url: authUrl });

  const link = page.locator(".oauth-link");
  await expect(link).toBeVisible({ timeout: 3000 });
  await expect(link).toHaveText("Open authorization page ↗");
  await expect(link).toHaveAttribute("href", authUrl);
  await expect(page.locator(".oauth-loading")).not.toBeVisible();
});

test("dialog shows waiting state after oauth_url arrives", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  await server.sendEvent({ type: "oauth_url", url: "https://claude.ai/oauth/authorize?test=1" });
  await expect(page.locator(".oauth-link")).toBeVisible({ timeout: 3000 });

  // The waiting indicator is shown (no code input)
  await expect(page.locator(".oauth-waiting")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".oauth-waiting")).toHaveText("Waiting for authorization…");
});

test("dialog closes automatically when server sends auth_mode_changed", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await seedApiKeyMode(server);

  const select = page.locator(".session-bar-select").first();
  await expect(select).toBeVisible({ timeout: 3000 });
  await select.selectOption("claude-max");

  await server.nextMessage(); // drain set_auth_mode

  await expect(page.locator(".oauth-overlay")).toBeVisible({ timeout: 3000 });

  // Server sends oauth_url (localhost callback server started)
  await server.sendEvent({ type: "oauth_url", url: "https://claude.ai/oauth/authorize?test=1" });
  await expect(page.locator(".oauth-link")).toBeVisible({ timeout: 3000 });

  // Server sends auth_mode_changed (callback received, exchange succeeded)
  await server.sendEvent({ type: "auth_mode_changed", authMode: "claude-max" });

  // Dialog closes automatically — no user action needed
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
