/**
 * Omega Web UI — copy button and diff rendering tests.
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Send a complete llm_response turn with the given markdown text. */
async function sendLlmResponse(server: any, text: string) {
  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_response",
    stopReason: "end_turn",
    contextHash: "abcd1234ef56",
    usage: { input_tokens: 10, output_tokens: 5 },
    text,
  });
}

// ---------------------------------------------------------------------------
// Copy button
// ---------------------------------------------------------------------------

test("copy button is present in the DOM for a fenced code block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```\nconsole.log('hello');\n```");

  // Button exists in the DOM (opacity:0 until hover)
  await expect(page.locator(".code-copy-btn")).toBeAttached({ timeout: 3000 });
});

test("copy button is visible on hover", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```\nconsole.log('hello');\n```");

  const pre = page.locator(".md-body pre");
  await pre.waitFor({ timeout: 3000 });
  await pre.hover();

  await expect(page.locator(".code-copy-btn")).toBeVisible({ timeout: 2000 });
});

test("copy button shows feedback after click", async ({ page, server }) => {
  // Mock clipboard so the test is deterministic in headless mode
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: () => Promise.resolve() },
      configurable: true,
    });
  });

  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```\nconsole.log('hello');\n```");

  const pre = page.locator(".md-body pre");
  await pre.waitFor({ timeout: 3000 });
  await pre.hover();

  const btn = page.locator(".code-copy-btn");
  await expect(btn).toBeVisible({ timeout: 2000 });
  await btn.click();

  // Button shows checkmark feedback
  await expect(btn).toHaveText("✓", { timeout: 2000 });
});

test("multiple code blocks each get their own copy button", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```\nfirst\n```\n\nsome text\n\n```\nsecond\n```");

  await expect(page.locator(".code-copy-btn")).toHaveCount(2, { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// Diff rendering
// ---------------------------------------------------------------------------

test("diff block: added lines get .diff-add class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n+ added line\n```");

  await expect(page.locator(".diff-add")).toBeAttached({ timeout: 3000 });
  await expect(page.locator(".diff-add")).toContainText("+ added line");
});

test("diff block: removed lines get .diff-del class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n- removed line\n```");

  await expect(page.locator(".diff-del")).toBeAttached({ timeout: 3000 });
  await expect(page.locator(".diff-del")).toContainText("- removed line");
});

test("diff block: hunk headers get .diff-hunk class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n@@ -1,3 +1,4 @@\n```");

  await expect(page.locator(".diff-hunk")).toBeAttached({ timeout: 3000 });
  await expect(page.locator(".diff-hunk")).toContainText("@@ -1,3 +1,4 @@");
});

test("diff block: file headers get .diff-file class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n--- a/foo.ts\n+++ b/foo.ts\n```");

  await expect(page.locator(".diff-file")).toHaveCount(2, { timeout: 3000 });
});

test("diff block: context lines get .diff-ctx class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n  unchanged line\n```");

  await expect(page.locator(".diff-ctx")).toBeAttached({ timeout: 3000 });
  await expect(page.locator(".diff-ctx")).toContainText("  unchanged line");
});

test("diff block: pre gets diff-block class", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n+ added\n- removed\n```");

  await expect(page.locator(".md-body pre.diff-block")).toBeAttached({ timeout: 3000 });
});

test("diff block: copy button still present alongside diff colouring", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```diff\n+ added\n- removed\n```");

  // Both the diff class and the copy button should exist on the same pre
  const pre = page.locator(".md-body pre.diff-block");
  await pre.waitFor({ timeout: 3000 });
  await expect(pre.locator(".code-copy-btn")).toBeAttached();
});

test("patch language tag also triggers diff rendering", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, "```patch\n+ added\n```");

  await expect(page.locator(".diff-add")).toBeAttached({ timeout: 3000 });
});
