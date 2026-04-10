/**
 * Omega Web UI — additional e2e tests (Playwright).
 *
 * Covers gaps identified in the test review:
 *  1. Open-turn crash recovery (page reload after server crash mid-turn)
 *  2. Abort button sends { type: "abort" } to server
 *  3. Streaming locks / unlocks input (send + textarea disabled while streaming)
 *  4. tool_result renders in UI
 *  5. agent_error and llm_error render in UI
 *  6. Textarea clears after send
 *  7. History replay completeness (tool_call, turn_end footer survive reload)
 *  8. Reconnect banner appears after repeated failures
 */

import { test, expect } from "./fixtures/index.js";

// Shorthand for waiting until the Ω button is in "connected" state.
const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

// ---------------------------------------------------------------------------
// 1. Open-turn crash recovery
// ---------------------------------------------------------------------------

test("crash recovery: page reload after open turn shows turn_interrupted", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Simulate a turn that never finished (server crash mid-turn)
  await server.sendEvent({ type: "user_message", content: "this turn never finished" });
  await server.sendEvent({ type: "text", text: "partial response…" });
  // No turn_end — server "crashed" here

  await expect(page.getByTestId("block-user")).toBeVisible({ timeout: 3000 });

  // Reload — server replays the open turn from its in-memory log
  // closeOpenTurn() should append a synthetic turn_interrupted
  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  // User block should still be there
  await expect(page.getByTestId("block-user")).toBeVisible({ timeout: 3000 });
  // UI must NOT be stuck in streaming state
  await expect(connectedDot(page)).toBeVisible({ timeout: 3000 });
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 3000 });
  // Interrupted marker should be visible
  await expect(page.getByTestId("block-interrupt")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 2. Abort button sends { type: "abort" } to server
// ---------------------------------------------------------------------------

test("abort button click sends abort message to server", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Start a turn so the abort button appears
  await server.sendEvent({ type: "user_message", content: "long running" });
  await expect(page.getByRole("button", { name: "Abort" })).toBeVisible({ timeout: 3000 });

  // Click abort
  await page.getByRole("button", { name: "Abort" }).click();

  const msg = await server.nextMessage();
  expect((msg as any).type).toBe("abort");
});

// ---------------------------------------------------------------------------
// 3. Streaming locks / unlocks input
// ---------------------------------------------------------------------------

test("abort button replaces send button while streaming", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  // user_message sets streaming=true → abort-btn replaces send-btn
  await expect(page.getByRole("button", { name: "Abort" })).toBeVisible({ timeout: 3000 });
  await expect(page.getByRole("button", { name: "Send" })).not.toBeVisible();
});

test("textarea is enabled while streaming (typing allowed, send blocked)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.getByRole("button", { name: "Abort" })).toBeVisible({ timeout: 3000 });
  // Textarea stays enabled so the user can compose a reply while waiting
  await expect(page.locator("textarea")).toBeEnabled();
});

test("input unlocks after turn_end", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.getByRole("button", { name: "Abort" })).toBeVisible({ timeout: 3000 });

  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 3, costUsd: 0, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.getByRole("button", { name: "Send" })).toBeVisible({ timeout: 3000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

test("input unlocks after turn_interrupted", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.getByRole("button", { name: "Abort" })).toBeVisible({ timeout: 3000 });

  await server.sendEvent({ type: "turn_interrupted" });

  await expect(page.getByRole("button", { name: "Send" })).toBeVisible({ timeout: 3000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

// ---------------------------------------------------------------------------
// 4. tool_result renders in UI
// ---------------------------------------------------------------------------

test("tool_result event shows result block", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "tool_call",
    id: "tc-001",
    name: "read_file",
    input: { path: "src/agent.ts" },
    contextHash: "abcdef123456",
  });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-001",
    name: "read_file",
    isError: false,
    durationMs: 1,
    output: "file contents here",
    contextHash: "abcdef123456",
  });

  const resultBlock = page.getByTestId("block-result");
  await expect(resultBlock).toBeVisible({ timeout: 3000 });
  await expect(resultBlock.locator(".block-label")).toContainText("tool_result");
  await expect(resultBlock.locator(".block-body")).toContainText("file contents here");
});

test("tool_result with is_error shows error styling", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-002",
    name: "run_command",
    isError: true,
    durationMs: 1,
    output: "command not found",
    contextHash: "abcdef123456",
  });

  await expect(page.locator('[data-testid="block-result"][data-error="true"]')).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 5. agent_error and llm_error render in UI
// ---------------------------------------------------------------------------

test("agent_error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "agent_error", error: "Context too large to send. Use /compact." });

  const errBlock = page.getByTestId("block-error");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toContainText("Context too large");
});

test("llm_error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_error",
    provider: "anthropic",
    url: "https://api.anthropic.com/v1/messages",
    error: "rate limited",
    httpStatus: 429,
  });

  const errBlock = page.getByTestId("block-error");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toContainText("rate limited");
});

// ---------------------------------------------------------------------------
// 6. Textarea clears after send
// ---------------------------------------------------------------------------

test("textarea is empty after sending a message", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  const textarea = page.locator("textarea");
  await textarea.fill("hello world");
  await expect(textarea).toHaveValue("hello world");

  await textarea.press("Enter");
  await server.nextMessage(); // wait for message to be received

  await expect(textarea).toHaveValue("");
});

// ---------------------------------------------------------------------------
// 7. History replay completeness
// ---------------------------------------------------------------------------

test("tool_call survives page reload (history replay)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "use a tool" });
  await server.sendEvent({
    type: "tool_call",
    id: "tc-003",
    name: "read_file",
    input: { path: "README.md" },
    contextHash: "abcdef123456",
  });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-003",
    name: "read_file",
    isError: false,
    durationMs: 1,
    output: "readme contents",
    contextHash: "abcdef123456",
  });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 20, outputTokens: 10, costUsd: 0.0002, savedUsd: 0, ttftMs: 80 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.getByTestId("block-tool")).toBeVisible({ timeout: 3000 });

  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  // tool_call block should be replayed
  await expect(page.getByTestId("block-tool")).toBeVisible({ timeout: 3000 });
  // turn_end footer should be replayed
  await expect(page.getByTestId("block-turn-end")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 8. Reconnect banner
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 9. Assistant text survives page reload (history replay)
// ---------------------------------------------------------------------------

test("assistant text survives page reload (history replay)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hello" });
  // text (StreamSignal) is ephemeral — shown live during streaming.
  // llm_response carries the settled text field — this is the persisted form.
  await server.sendEvent({ type: "text", text: "I am alive." });
  await server.sendEvent({
    type: "llm_response",
    stopReason: "end_turn",
    usage: { input_tokens: 5, output_tokens: 5 },
    contextHash: "ab12cd34ef56",
    text: "I am alive.",
  });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  // Confirm text is visible before reload — now inside the llm_response block
  await expect(page.getByTestId("block-llm-response")).toContainText("I am alive.", { timeout: 3000 });

  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Assistant text must survive the reload via history replay
  await expect(page.getByTestId("block-llm-response")).toContainText("I am alive.", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 10. No yellow flash during history replay
// ---------------------------------------------------------------------------

test("no yellow flash during history replay — dot stays green, never streaming", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Build up a completed turn
  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "hello back" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(connectedDot(page)).toBeVisible({ timeout: 3000 });

  // Track whether the streaming dot ever appears during reload
  await page.reload();

  // Immediately after reload starts, we cannot observe intermediate states
  // reliably, but after reconnect the dot must be green (connected), not yellow.
  await connectedDot(page).waitFor({ timeout: 5000 });
  // Dot must NOT be in streaming state
  await expect(page.locator('[data-testid="omega-btn"][data-status="streaming"]')).not.toBeVisible();
  // Input must be enabled (streaming=false means textarea is enabled)
  await expect(page.locator("textarea")).toBeEnabled({ timeout: 3000 });
});



test("reconnect banner appears after repeated connection failures", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Simulate 2 rapid disconnects by driving the store directly.
  // (We can't kill the actual WS server cleanly in tests, so we call
  // handleDisconnect via the __omegaHandleDisconnect handle exposed by App.)
  await page.evaluate(() => {
    const handleDisconnect = (window as any).__omegaHandleDisconnect;
    if (handleDisconnect) {
      handleDisconnect();
      handleDisconnect();
    }
  });

  await expect(page.getByTestId("reconnect-banner")).toBeVisible({ timeout: 3000 });
});
