/**
 * Stage 3 UI-contract tests for pause/resume/interject.
 *
 * These tests drive turnState transitions by injecting events via the control
 * HTTP API (which the test server uses to broadcast session_info with the
 * derived turnState). They validate:
 *
 *   - Status label/dot transitions across Running \u2192 PauseRequested \u2192 Paused
 *   - Button matrix per state (Send/Pause/Continue/Take it back/Abort)
 *   - Keyboard shortcuts (Esc in Running \u2192 pause; Esc in PauseRequested \u2192 abort;
 *     Enter in Paused with/without text \u2192 continue with or without interjection)
 *   - Pre-commit visual state and drain-on-paused behaviour (Continue pressed
 *     from PauseRequested arms preCommitted; when turnState flips to paused,
 *     the client auto-sends the continue message)
 *   - Take it back clears the armed pre-commit
 *   - Reconnect (via __omegaHandleDisconnect) clears preCommitted
 *
 * Stage 4 will add integration tests that drive the real server end-to-end.
 */

import { test, expect } from "./fixtures";
import type { Page } from "@playwright/test";

function connectedDot(page: Page) {
  return page.locator('[data-testid="omega-btn"][data-status="connected"]');
}

// -- helpers ----------------------------------------------------------------

async function waitForTurnState(page: Page, expected: string): Promise<void> {
  await expect
    .poll(
      () => page.getAttribute('[data-testid="omega-btn"]', "data-status"),
      { timeout: 3000 },
    )
    .toBe(expected);
}

// ---------------------------------------------------------------------------
// 1. Status label cycles through all turn states
// ---------------------------------------------------------------------------

test("status label cycles through Ready \u2192 Streaming \u2192 Pausing \u2192 Paused", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await expect(page.getByTestId("status-label")).toHaveText("Ready");

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.getByTestId("status-label")).toHaveText("Streaming\u2026", { timeout: 3000 });

  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("status-label")).toHaveText("Pausing\u2026", { timeout: 3000 });

  await server.sendEvent({ type: "turn_paused", summary: "paused at seam" });
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 3000 });

  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 3, costUsd: 0, savedUsd: 0, ttftMs: 10 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// 2. Button matrix per turnState
// ---------------------------------------------------------------------------

test("button matrix: Idle shows Send; Running shows Pause; PauseRequested/Paused show Continue + Abort", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Idle
  await expect(page.getByTestId("send-btn")).toBeVisible();
  await expect(page.getByTestId("pause-btn")).not.toBeVisible();
  await expect(page.getByTestId("continue-btn")).not.toBeVisible();
  await expect(page.getByTestId("abort-btn")).not.toBeVisible();
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible();

  // Running
  await server.sendEvent({ type: "user_message", content: "hi" });
  await waitForTurnState(page, "streaming");
  await expect(page.getByTestId("send-btn")).not.toBeVisible();
  await expect(page.getByTestId("pause-btn")).toBeVisible();
  await expect(page.getByTestId("continue-btn")).not.toBeVisible();
  await expect(page.getByTestId("abort-btn")).not.toBeVisible();

  // PauseRequested
  await server.sendEvent({ type: "pause_requested" });
  await waitForTurnState(page, "pause-requested");
  await expect(page.getByTestId("send-btn")).not.toBeVisible();
  await expect(page.getByTestId("pause-btn")).not.toBeVisible();
  await expect(page.getByTestId("continue-btn")).toBeVisible();
  await expect(page.getByTestId("abort-btn")).toBeVisible();
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible();

  // Paused
  await server.sendEvent({ type: "turn_paused", summary: "at seam" });
  await waitForTurnState(page, "paused");
  await expect(page.getByTestId("send-btn")).not.toBeVisible();
  await expect(page.getByTestId("pause-btn")).not.toBeVisible();
  await expect(page.getByTestId("continue-btn")).toBeVisible();
  await expect(page.getByTestId("abort-btn")).toBeVisible();
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible();
});

// ---------------------------------------------------------------------------
// 3. Esc while Running sends pause
// ---------------------------------------------------------------------------

test("Esc from Running sends pause message", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await expect(page.getByTestId("pause-btn")).toBeVisible({ timeout: 3000 });

  await page.locator("textarea").focus();
  await page.keyboard.press("Escape");

  // Drain pending messages until we see pause
  for (let i = 0; i < 5; i++) {
    const msg = await server.nextMessage();
    if ((msg as any).type === "pause") return;
  }
  throw new Error("Did not receive pause message");
});

// ---------------------------------------------------------------------------
// 4. Esc while PauseRequested sends abort
// ---------------------------------------------------------------------------

test("Esc from PauseRequested sends abort", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  await page.locator("textarea").focus();
  await page.keyboard.press("Escape");

  for (let i = 0; i < 5; i++) {
    const msg = await server.nextMessage();
    if ((msg as any).type === "abort") return;
  }
  throw new Error("Did not receive abort message");
});

// ---------------------------------------------------------------------------
// 5. Enter from Paused with text sends continue with interjection
// ---------------------------------------------------------------------------

test("Enter from Paused with text sends continue{content}", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await server.sendEvent({ type: "turn_paused", summary: "seam" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  await page.locator("textarea").fill("try a different approach");
  await page.locator("textarea").press("Enter");

  let got: any = null;
  for (let i = 0; i < 5; i++) {
    const msg = await server.nextMessage();
    if ((msg as any).type === "continue") { got = msg; break; }
  }
  expect(got).not.toBeNull();
  expect(got.content).toBe("try a different approach");

  // Textarea cleared after send
  await expect(page.locator("textarea")).toHaveValue("");
});

// ---------------------------------------------------------------------------
// 6. Pre-commit visual: click Continue from PauseRequested \u2192 Take it back + new label
// ---------------------------------------------------------------------------

test("Continue from PauseRequested arms pre-commit (Take it back button + Pausing, will continue)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  // Click Continue \u2014 should NOT send a WS message yet; just arms pre-commit.
  await page.getByTestId("continue-btn").click();

  await expect(page.getByTestId("takeitback-btn")).toBeVisible({ timeout: 1000 });
  await expect(page.getByTestId("continue-btn")).not.toBeVisible();
  await expect(page.getByTestId("abort-btn")).toBeVisible();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing, will continue");

  // Verify: no continue message has been sent yet. Give it a beat.
  await page.waitForTimeout(200);
  const msgs = await server.drainMessages();
  const continues = msgs.map(m => JSON.parse(m)).filter(m => m.type === "continue");
  expect(continues).toHaveLength(0);
});

// ---------------------------------------------------------------------------
// 7. Pre-commit drain: after turn_paused arrives, client auto-sends continue
// ---------------------------------------------------------------------------

test("pre-commit drains on paused transition (sends continue with typed interjection)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  // Arm pre-commit and type an interjection while still pausing.
  await page.getByTestId("continue-btn").click();
  await page.locator("textarea").fill("keep going but quieter");
  await expect(page.getByTestId("takeitback-btn")).toBeVisible();

  // Server transitions to paused \u2014 client should auto-send continue.
  await server.sendEvent({ type: "turn_paused", summary: "seam" });

  let got: any = null;
  for (let i = 0; i < 5; i++) {
    const msg = await server.nextMessage();
    if ((msg as any).type === "continue") { got = msg; break; }
  }
  expect(got).not.toBeNull();
  expect(got.content).toBe("keep going but quieter");

  // Textarea cleared after auto-send
  await expect(page.locator("textarea")).toHaveValue("");
});

// ---------------------------------------------------------------------------
// 8. Take it back clears pre-commit; subsequent paused transition does NOT auto-send
// ---------------------------------------------------------------------------

test("Take it back cancels the armed pre-commit", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("takeitback-btn")).toBeVisible();

  await page.getByTestId("takeitback-btn").click();
  // Back to the un-armed PauseRequested view.
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 1000 });
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing\u2026");

  // Drain any pending messages, then drive paused \u2014 no auto-continue should fire.
  await server.drainMessages();
  await server.sendEvent({ type: "turn_paused", summary: "seam" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  await page.waitForTimeout(300);
  const msgs = await server.drainMessages();
  const continues = msgs.map(m => JSON.parse(m)).filter(m => m.type === "continue");
  expect(continues).toHaveLength(0);
});

// ---------------------------------------------------------------------------
// 9. Disconnect clears pre-commit
// ---------------------------------------------------------------------------

test("WS disconnect clears pre-commit (no lingering auto-send)", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({ type: "pause_requested" });
  await expect(page.getByTestId("continue-btn")).toBeVisible({ timeout: 3000 });

  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("takeitback-btn")).toBeVisible();

  // Simulate a WS drop. handleDisconnect() resets turnState=idle and
  // preCommitted=false; the authoritative state will arrive on the next
  // session_info once the client reconnects. For this test we only care that
  // the pre-commit is cleared — no lingering auto-send armed.
  await page.evaluate(() => {
    const fn = (window as any).__omegaHandleDisconnect;
    if (fn) fn();
  });

  // After disconnect, turnState is reset to idle and preCommitted is cleared.
  // The button matrix shows Send (no Take it back, no Continue).
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("continue-btn")).not.toBeVisible();
});
