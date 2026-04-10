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

test("Ω button is visible", async ({ page, server }) => {
  await page.goto("/");
  await expect(page.getByTestId("omega-btn")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Connection lifecycle
// ---------------------------------------------------------------------------

test("shows 'ready' status after connecting to server", async ({ page, server }) => {
  await page.goto("/");
  // Server sends `ready` event after history batch → state.connected = true
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 5000 });
});

test("status dot is green when connected", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });
});


// ---------------------------------------------------------------------------
// Input area
// ---------------------------------------------------------------------------

test("textarea is enabled when connected", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

test("send button is visible when connected and not streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Sending a message
// ---------------------------------------------------------------------------

test("sending a message — browser sends JSON to server", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  const textarea = page.locator("textarea");
  await textarea.fill("hello world");
  await textarea.press("Enter");

  const msg = await server.nextMessage();
  expect((msg as any).type).toBe("message");
  expect((msg as any).content).toBe("hello world");
});

test("user_message event shows user block in feed", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hello world" });
  await expect(page.getByTestId("block-user")).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("block-user").locator(".block-label")).toHaveText("user_message");
});

// ---------------------------------------------------------------------------
// Turn rendering
// ---------------------------------------------------------------------------

test("turn_end shows footer block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "Hello!" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 10, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 100 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.getByTestId("block-turn-end")).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("block-llm-response")).toBeVisible();
});

test("text event shows assistant block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "Hello from Omega!" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 10, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 100 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  const assistBlock = page.getByTestId("block-llm-response").locator(".block-body");
  await expect(assistBlock).toHaveText("Hello from Omega!", { timeout: 3000 });
});

test("tool_call event shows tool block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "tool_call",
    id: "tc-001",
    name: "read_file",
    input: { path: "src/agent.ts" },
    contextHash: "abcdef123456",
  });

  const toolBlock = page.getByTestId("block-tool");
  await expect(toolBlock).toBeVisible({ timeout: 3000 });
  await expect(toolBlock).toContainText("read_file");
});

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

test("transport_error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "transport_error", error: "Something went wrong" });

  const errBlock = page.getByTestId("block-error");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toHaveText("Something went wrong");
});

// ---------------------------------------------------------------------------
// Renderer parity — WEB-4
// ---------------------------------------------------------------------------

test("model_changed event shows status block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "model_changed", provider: "anthropic", model: "claude-opus-4-6" });

  const statusBlock = page.getByTestId("block-status");
  await expect(statusBlock).toBeVisible({ timeout: 3000 });
  await expect(statusBlock.locator(".block-body")).toHaveText("Switched to claude-opus-4-6");
});

test("llm_call event shows an api-call block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_call",
    url: "https://api.anthropic.com/v1/messages",
    model: "claude-sonnet-4-6",
    contextHashes: [],
    cacheBreakpointIndex: null,
    requestBytes: 0,
  });

  const block = page.getByTestId("block-llm-call");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block.locator(".block-label")).toContainText("llm_call");
});

test("llm_response event shows an api-response block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_response",
    stopReason: "end_turn",
    usage: { input_tokens: 100, output_tokens: 50 },
    contextHash: "abcdef123456",
  });

  const block = page.getByTestId("block-llm-response");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block.locator(".block-label")).toContainText("llm_response");
});

test("turn_interrupted event shows interrupt block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "turn_interrupted" });

  const block = page.getByTestId("block-interrupt");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block).toContainText("Interrupted");
});

test("turn_interrupted with reason=aborted shows '⊘ Aborted'", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "turn_interrupted", reason: "aborted" });

  const block = page.getByTestId("block-interrupt");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block).toContainText("Aborted");
});

test("turn_interrupted with reason=error shows '⊘ Failed'", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "turn_interrupted", reason: "error" });

  const block = page.getByTestId("block-interrupt");
  await expect(block).toBeVisible({ timeout: 3000 });
  await expect(block).toContainText("Failed");
});

test("llm_retry event changes status dot to 'retrying…'", async ({ page, server }) => {
  await page.goto("/");
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  // Status should be streaming at this point
  await expect(page.getByTestId("status-row")).toContainText("Streaming", { timeout: 3000 });

  await server.sendEvent({
    type: "llm_retry",
    attempt: 1,
    provider: "anthropic",
    waitMs: 5000,
    error: "overloaded",
  });
  await expect(page.getByTestId("status-row")).toContainText("Retrying", { timeout: 3000 });

  // After turn_interrupted the status returns to ready
  await server.sendEvent({ type: "turn_interrupted", reason: "error" });
  await expect(page.getByTestId("status-row")).toContainText("Ready", { timeout: 3000 });
});
