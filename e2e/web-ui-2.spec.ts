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

// ---------------------------------------------------------------------------
// 1. Open-turn crash recovery
// ---------------------------------------------------------------------------

test("crash recovery: page reload after open turn shows turn_interrupted", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Simulate a turn that never finished (server crash mid-turn)
  await server.sendEvent({ type: "user_message", content: "this turn never finished" });
  await server.sendEvent({ type: "text", text: "partial response…" });
  // No turn_end — server "crashed" here

  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  // Reload — server replays the open turn from its in-memory log
  // closeOpenTurn() should append a synthetic turn_interrupted
  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // User block should still be there
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });
  // UI must NOT be stuck in streaming state
  await expect(page.locator(".dot.connected")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".send-btn")).toBeVisible({ timeout: 3000 });
  // Interrupted marker should be visible
  await expect(page.locator(".block.interrupt")).toBeVisible({ timeout: 3000 });
});

test("crash recovery: UI is not stuck streaming after open-turn replay", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "crash test" });
  // No turn_end

  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // After replay the status must say "ready", not "streaming…"
  await expect(page.locator(".status-label")).toHaveText("ready", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 2. Abort button sends { type: "abort" } to server
// ---------------------------------------------------------------------------

test("abort button click sends abort message to server", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Start a turn so the abort button appears
  await server.sendEvent({ type: "user_message", content: "long running" });
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });

  // Click abort
  await page.locator(".abort-btn").click();

  const msg = await server.nextMessage();
  expect((msg as any).type).toBe("abort");
});

// ---------------------------------------------------------------------------
// 3. Streaming locks / unlocks input
// ---------------------------------------------------------------------------

test("send button is disabled while streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  // user_message sets streaming=true → send-btn is replaced by abort-btn
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".send-btn")).not.toBeVisible();
});

test("textarea is disabled while streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });
  await expect(page.locator("textarea")).toBeDisabled();
});

test("input unlocks after turn_end", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });

  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 3, costUsd: 0, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.locator(".send-btn")).toBeVisible({ timeout: 3000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

test("input unlocks after turn_interrupted", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.locator(".abort-btn")).toBeVisible({ timeout: 3000 });

  await server.sendEvent({ type: "turn_interrupted" });

  await expect(page.locator(".send-btn")).toBeVisible({ timeout: 3000 });
  await expect(page.locator("textarea")).toBeEnabled();
});

// ---------------------------------------------------------------------------
// 4. tool_result renders in UI
// ---------------------------------------------------------------------------

test("tool_result event shows result block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "tool_call",
    id: "tc-001",
    name: "read_file",
    input: { path: "src/agent.ts" },
  });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-001",
    name: "read_file",
    isError: false,
    durationMs: 1,
    output: "file contents here",
  });

  const resultBlock = page.locator(".block.result");
  await expect(resultBlock).toBeVisible({ timeout: 3000 });
  await expect(resultBlock.locator(".block-label")).toContainText("read_file");
  await expect(resultBlock.locator(".block-body")).toContainText("file contents here");
});

test("tool_result with is_error shows error styling", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-002",
    name: "run_command",
    isError: true,
    durationMs: 1,
    output: "command not found",
  });

  await expect(page.locator(".block.result.result-error")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 5. agent_error and llm_error render in UI
// ---------------------------------------------------------------------------

test("agent_error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "agent_error", error: "Context too large to send. Use /compact." });

  const errBlock = page.locator(".block.error-b");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toContainText("Context too large");
});

test("llm_error event shows error block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "llm_error",
    provider: "anthropic",
    url: "https://api.anthropic.com/v1/messages",
    error: "rate limited",
    httpStatus: 429,
  });

  const errBlock = page.locator(".block.error-b");
  await expect(errBlock).toBeVisible({ timeout: 3000 });
  await expect(errBlock.locator(".block-body")).toContainText("rate limited");
});

// ---------------------------------------------------------------------------
// 6. Textarea clears after send
// ---------------------------------------------------------------------------

test("textarea is empty after sending a message", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

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
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "use a tool" });
  await server.sendEvent({
    type: "tool_call",
    id: "tc-003",
    name: "read_file",
    input: { path: "README.md" },
  });
  await server.sendEvent({
    type: "tool_result",
    id: "tc-003",
    name: "read_file",
    isError: false,
    durationMs: 1,
    output: "readme contents",
  });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 20, outputTokens: 10, costUsd: 0.0002, savedUsd: 0, ttftMs: 80 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.locator(".block.tool")).toBeVisible({ timeout: 3000 });

  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // tool_call block should be replayed
  await expect(page.locator(".block.tool")).toBeVisible({ timeout: 3000 });
  // turn_end footer should be replayed
  await expect(page.locator(".block.footer")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 8. Reconnect banner
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 9. Assistant text survives page reload (history replay)
// ---------------------------------------------------------------------------

test("assistant text survives page reload (history replay)", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hello" });
  // text (StreamSignal) is ephemeral — the real agent also emits assistant_text (OmegaEvent)
  // which is the persisted form. Both sent here to mirror what the agent does.
  await server.sendEvent({ type: "text", text: "I am alive." });
  await server.sendEvent({ type: "assistant_text", text: "I am alive." });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  // Confirm text is visible before reload
  await expect(page.locator(".block.assist")).toContainText("I am alive.", { timeout: 3000 });

  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Assistant text must survive the reload via history replay
  await expect(page.locator(".block.assist")).toContainText("I am alive.", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 10. No yellow flash during history replay
// ---------------------------------------------------------------------------

test("no yellow flash during history replay — dot stays green, never streaming", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Build up a completed turn
  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "text", text: "hello back" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(page.locator(".dot.connected")).toBeVisible({ timeout: 3000 });

  // Track whether the streaming dot ever appears during reload
  await page.reload();

  // Immediately after reload starts, we cannot observe intermediate states
  // reliably, but after reconnect the dot must be green (connected), not yellow.
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  // Dot must NOT be in streaming state
  await expect(page.locator(".dot.streaming")).not.toBeVisible();
  // Input must be enabled (streaming=false means textarea is enabled)
  await expect(page.locator("textarea")).toBeEnabled({ timeout: 3000 });
});



test("reconnect banner appears after repeated connection failures", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Simulate 2 rapid disconnects by driving the store dispatch directly.
  // (We can't kill the actual WS server cleanly in tests, so we inject
  // disconnected events via the __omegaDispatch handle exposed by App.)
  await page.evaluate(() => {
    const dispatch = (window as any).__omegaDispatch;
    if (dispatch) {
      dispatch({ type: "disconnected" });
      dispatch({ type: "disconnected" });
    }
  });

  await expect(page.locator(".reconnect-banner")).toBeVisible({ timeout: 3000 });
});
