/**
 * Playwright test for the llm_call "messages" modal and the /context endpoint.
 *
 * The /context endpoint is mocked via page.route() — the unit tests in
 * src/web/context-lookup.test.ts cover the server-side lookupContextRecords
 * logic. This test covers the browser-side behaviour:
 *   - clicking the "messages" button opens the modal
 *   - the correct hashes are sent to /context
 *   - the returned records are rendered in the modal
 */

import { test, expect } from "./fixtures/index.js";

const HASH_1 = "aabbccddeeff";
const HASH_2 = "112233445566";

test("llm_call messages modal fetches /context and renders records", async ({ page, server }) => {
  // Intercept the /context fetch before navigating so we don't miss it.
  const requestedHashes: string[] = [];
  await page.route("**/context**", async (route) => {
    const url = new URL(route.request().url());
    const raw = url.searchParams.get("hashes") ?? "";
    requestedHashes.push(...raw.split(",").filter(Boolean));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        { hash: HASH_1, time: "2025-01-01T00:00:00.000Z", role: "user",      content: "What is 2+2?" },
        { hash: HASH_2, time: "2025-01-01T00:00:01.000Z", role: "assistant", content: "It is 4."    },
      ]),
    });
  });

  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Send a turn with an llm_call carrying two context hashes.
  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_call",
    url: "https://api.anthropic.com/v1/messages",
    model: "claude-sonnet-4-6",
    contextHashes: [HASH_1, HASH_2],
    cacheBreakpointIndex: 1,
    requestBytes: 512,
  });

  // Click the "messages" button on the llm_call block.
  const messagesBtn = page.locator(".block.api-call .block-expand-btn", { hasText: /^messages/ });
  await expect(messagesBtn).toBeVisible({ timeout: 3000 });
  await messagesBtn.click();

  // Modal must open.
  const modal = page.locator(".llm-call-modal");
  await expect(modal).toBeVisible({ timeout: 3000 });

  // The /context endpoint must have been called with both hashes.
  expect(requestedHashes).toContain(HASH_1);
  expect(requestedHashes).toContain(HASH_2);

  // Both messages must appear in the modal (rendered newest-first).
  const bodies = page.locator(".llm-call-msg-body");
  await expect(bodies).toHaveCount(2, { timeout: 3000 });

  // Newest record (HASH_2 — assistant) is rendered first.
  await expect(bodies.nth(0)).toHaveText("It is 4.");
  await expect(bodies.nth(1)).toHaveText("What is 2+2?");

  // Role labels are present.
  const roles = page.locator(".llm-call-msg-role");
  await expect(roles.nth(0)).toContainText("assistant");
  await expect(roles.nth(1)).toContainText("user");
});
