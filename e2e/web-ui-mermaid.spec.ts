/**
 * Omega Web UI — Mermaid diagram rendering tests.
 *
 * Mermaid is loaded lazily on first use, so each test waits for the
 * `.mermaid-wrapper` to appear before making assertions.
 */

import { test, expect } from "./fixtures/index.js";

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

const SIMPLE_DIAGRAM = "```mermaid\ngraph LR\n  A --> B\n```";
const INVALID_DIAGRAM = "```mermaid\nthis is not valid mermaid syntax !!!\n```";

// ---------------------------------------------------------------------------
// Successful render
// ---------------------------------------------------------------------------

test("mermaid block renders an SVG diagram", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, SIMPLE_DIAGRAM);

  // Wrapper replaces the <pre>
  const wrapper = page.locator(".mermaid-wrapper");
  await wrapper.waitFor({ timeout: 10000 });

  // An SVG should be inside the diagram div
  const svg = page.locator(".mermaid-diagram svg");
  await expect(svg).toBeAttached({ timeout: 5000 });
});

test("mermaid block: copy button is present in the DOM", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, SIMPLE_DIAGRAM);

  const wrapper = page.locator(".mermaid-wrapper");
  await wrapper.waitFor({ timeout: 10000 });

  await expect(wrapper.locator(".code-copy-btn")).toBeAttached();
});

test("mermaid block: copy button is visible on hover", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, SIMPLE_DIAGRAM);

  const wrapper = page.locator(".mermaid-wrapper");
  await wrapper.waitFor({ timeout: 10000 });
  await wrapper.hover();

  await expect(wrapper.locator(".code-copy-btn")).toBeVisible({ timeout: 2000 });
});

// ---------------------------------------------------------------------------
// Error fallback
// ---------------------------------------------------------------------------

test("invalid mermaid: shows error notice", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, INVALID_DIAGRAM);

  const wrapper = page.locator(".mermaid-wrapper");
  await wrapper.waitFor({ timeout: 10000 });

  await expect(page.locator(".mermaid-error-notice")).toBeAttached({ timeout: 5000 });
  await expect(page.locator(".mermaid-error-notice")).toContainText("⚠ Mermaid error");
});

test("invalid mermaid: raw source is shown alongside the error", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await sendLlmResponse(server, INVALID_DIAGRAM);

  const wrapper = page.locator(".mermaid-wrapper");
  await wrapper.waitFor({ timeout: 10000 });

  // The .mermaid-source pre should contain the raw diagram text
  await expect(page.locator(".mermaid-source")).toBeAttached({ timeout: 5000 });
  await expect(page.locator(".mermaid-source")).toContainText("this is not valid mermaid syntax");
});
