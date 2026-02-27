/**
 * Omega Web UI — end-to-end tests (Playwright).
 *
 * Run: npx playwright test
 *   or: just e2e   (also builds frontend first)
 *
 * The test server (e2e/fixtures/test-server.ts) is started automatically
 * by Playwright's webServer config as a Bun subprocess on port 3001.
 * Tests control it via the control HTTP API on port 3002.
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// Page load
// ---------------------------------------------------------------------------

test("page title is 'Omega'", async ({ page, server }) => {
  await page.goto("/");
  await expect(page).toHaveTitle(/Omega/i);
});

test("heading 'Ω Omega' is visible", async ({ page, server }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: /Omega/i })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Connection lifecycle
// ---------------------------------------------------------------------------

test("shows 'ready' status after connecting to server", async ({ page, server }) => {
  await page.goto("/");
  // Server sends `connected` event on WS open → state.connected = true
  await expect(page.locator(".status-label")).toHaveText("ready", { timeout: 5000 });
});

test("status dot is green when connected", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
});

test("shows auth mode label after auth event", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await server.sendEvent({ type: "auth", mode: "oauth (test)" });
  await expect(page.locator(".model-label")).toHaveText("oauth (test)", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// Input area
// ---------------------------------------------------------------------------

test("textarea is enabled when connected", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

test("send button is visible when connected and not streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await expect(page.locator(".send-btn")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Sending a message
// ---------------------------------------------------------------------------

test("sending a message — browser sends JSON to server", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  const textarea = page.locator("textarea");
  await textarea.fill("hello world");
  await textarea.press("Enter");

  const msg = await server.nextMessage();
  expect((msg as any).type).toBe("message");
  expect((msg as any).content).toBe("hello world");
});

test("user_message event shows user block in feed", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hello world" });
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".block.user .block-body")).toHaveText("hello world");
});

test("abort button appears when streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Simulate a turn starting (server echoes user_message which sets streaming=true)
  await server.sendEvent({ type: "user_message", content: "test prompt" });
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// Turn rendering
// ---------------------------------------------------------------------------

test("turn_end shows footer block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "Hello!" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 10, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 100 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await server.sendEvent({ type: "turn_ready" });

  await expect(page.locator(".block.footer")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".block.assist")).toBeVisible();
});

test("text event shows assistant block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "Hello from Omega!" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 10, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 100 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await server.sendEvent({ type: "turn_ready" });

  const assistBlock = page.locator(".block.assist .block-body");
  await expect(assistBlock).toHaveText("Hello from Omega!", { timeout: 3000 });
});

test("agent_to_agent_tool_call event shows tool block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "agent_to_agent_tool_call",
    id: "tc-001",
    name: "read_file",
    input: { path: "src/agent.ts" },
  });

  const toolBlock = page.locator(".block.tool");
  await expect(toolBlock).toBeVisible({ timeout: 3000 });
  await expect(toolBlock.locator(".block-label")).toContainText("read_file");
});

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

test("error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "error", error: "Something went wrong" });
  await server.sendEvent({ type: "turn_ready" });

  const errBlock = page.locator(".block.error-b");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toHaveText("Something went wrong");
});

// ---------------------------------------------------------------------------
// Renderer parity — WEB-4
// ---------------------------------------------------------------------------

test("status event shows status block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "status", message: "thinking..." });

  const statusBlock = page.locator(".block.status");
  await expect(statusBlock).toBeVisible({ timeout: 3000 });
  await expect(statusBlock.locator(".block-body")).toHaveText("thinking...");
});

test("world_state_saved event shows a status pill", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "world_state_saved", path: "plan/world-state.md", charCount: 1234 });

  const pill = page.locator(".block.world-state-saved");
  await expect(pill).toBeVisible({ timeout: 3000 });
  await expect(pill).toContainText("world state saved");
});

test("llm_call event shows a collapsible api-call block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_call",
    llmCallNumber: 1,
    provider: "anthropic",
    url: "https://api.anthropic.com/v1/messages",
    request: { model: "claude-sonnet-4-6", max_tokens: 8192 },
  });

  const block = page.locator(".block.api-call");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block.locator(".block-label")).toContainText("llm call");
});

test("llm_to_agent event shows an api-response block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_to_agent",
    provider: "anthropic",
    url: "https://api.anthropic.com/v1/messages",
    stopReason: "end_turn",
    usage: { input_tokens: 100, output_tokens: 50 },
    content: [],
  });

  const block = page.locator(".block.api-response");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block.locator(".block-label")).toContainText("api response");
});

test("turn_interrupted event shows interrupt block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "turn_interrupted" });

  const block = page.locator(".block.interrupt");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block).toContainText("Interrupted");
});

// ---------------------------------------------------------------------------
// History replay (browser refresh)
// ---------------------------------------------------------------------------

test("history is replayed after page reload", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Send a complete turn
  await server.sendEvent({ type: "user_message", content: "replay test" });
  await server.sendEvent({ type: "text", text: "I remember you." });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await server.sendEvent({ type: "turn_ready" });

  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  // Reload the page — server should replay the event log
  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // User block should still be visible (replayed from server event log)
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".block.user .block-body")).toHaveText("replay test");
});
