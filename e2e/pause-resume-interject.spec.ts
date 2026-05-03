/**
 * Stage 4 integration tests for pause/resume/interject.
 *
 * Runs against the real-server fixture (e2e/fixtures/real-server.ts) which
 * starts the production runWebApp() with a mock CreateMessageStream and
 * exposes an LLM-call history on port 3004. Each test creates a fresh
 * session and resets the LLM call history to stay isolated from its
 * siblings.
 *
 * Scenarios:
 *   1. Multi-tool turn: pause during 2nd tool; seam only after that result.
 *   2. Concurrent tools: pause waits for all tools to complete.
 *   3. Pause + browser reload + manual continue with interjection.
 *   4. Pre-commit + browser reload: pre-commit drops; turnState survives.
 *   5. Two pauses in one turn (with interjections on each).
 *   6. Pause during pure-text LLM stream: no truncation.
 *   7. Session resume basis includes interjection as "User (mid-turn): ...".
 *   8. Reload during Paused must never flash a turn_interrupted block
 *      (regression: real session 2026-04-20T06-29-36 briefly showed
 *      "⊘ Interrupted" in the feed after refresh while paused).
 */

import { test, expect, type Page } from "@playwright/test";
import {
  SCRIPTS,
  getCalls,
  loadScript,
  resetCalls,
} from "./fixtures/real-server-control";

const connectedDot = (page: Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

/**
 * Wait until the WS connection is alive and the client has processed the
 * initial session_info broadcast — regardless of current turnState. Use
 * after `page.reload()` where the server's turnState may be `paused` or
 * `pause_requested`, so `connectedDot` (which matches only the idle
 * `data-status="connected"`) would never resolve.
 */
async function waitForAlive(page: Page, timeoutMs = 5000): Promise<void> {
  await page.waitForFunction(() => {
    const btn = document.querySelector('[data-testid="omega-btn"]');
    const status = btn?.getAttribute("data-status");
    return !!status
      && status !== "connecting"
      && status !== "disconnected"
      && status !== "retrying";
  }, undefined, { timeout: timeoutMs });
}

async function openNewSession(page: Page): Promise<void> {
  const picker = page.getByTestId("session-picker-modal");
  const pickerAlreadyOpen = await picker.isVisible().catch(() => false);
  if (!pickerAlreadyOpen) {
    await page.getByTestId("sessions-btn").click();
    await expect(picker).toBeVisible({ timeout: 5000 });
  }
  // Wait until the New session button is enabled (idle state).
  await expect(page.getByTestId("session-picker-new")).toBeEnabled({ timeout: 10000 });

  // Capture the current session dir BEFORE clicking, so we can wait for the
  // new session_info to arrive and update data-session-dir. Waiting for
  // status="Ready" alone is insufficient: if the server was already idle on
  // a previous session when we arrive, status is already "Ready" and that
  // wait passes instantly — before the reset has roundtripped. The test
  // then reads data-session-dir and gets the stale previous value.
  const prevDir = await page
    .getByTestId("sessions-btn")
    .getAttribute("data-session-dir");

  await page.getByTestId("session-picker-new").click();
  await expect(picker).not.toBeVisible({ timeout: 5000 });
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 5000 });

  // Wait for the server's session_info for the new session to be applied.
  await expect
    .poll(
      async () =>
        await page.getByTestId("sessions-btn").getAttribute("data-session-dir"),
      { timeout: 5000 },
    )
    .not.toBe(prevDir);
}

interface ProjectedCall {
  systemKind: "task" | "resumption";
  at: number;
  messages: Array<{ role: string; content: string }>;
}

// ---------------------------------------------------------------------------

test.beforeEach(async () => {
  await resetCalls();
});

// 1.
test("multi-tool: pause during 2nd tool; turn_paused only after its result; interjection reaches next LLM call", async ({ page }) => {
  await loadScript(SCRIPTS.multiTool());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("MULTI_TOOL_TEST kick off");
  await page.locator("textarea").press("Enter");

  // Wait until 2 tool blocks are visible — means the 2nd tool_call has
  // arrived; its result is still outstanding (sleep 0.6).
  await expect(page.getByTestId("block-tool")).toHaveCount(2, { timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing…", { timeout: 2000 });
  // turn_paused arrives only after the 2nd tool_result lands at the seam.
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });

  await page.locator("textarea").fill("use fewer tools");
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });

  const calls = await getCalls();
  const gotInterjection = calls.some(
    c =>
      c.systemKind === "task" &&
      c.messages.some(m => m.role === "user" && m.content.includes("use fewer tools")),
  );
  expect(gotInterjection).toBe(true);
});

// 2.
test("concurrent tools: pause waits for the slow tool before turn_paused", async ({ page }) => {
  await loadScript(SCRIPTS.concurrentTools());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("CONCURRENT_TOOLS_TEST go");
  await page.locator("textarea").press("Enter");

  // Both tool blocks appear together (emitted in one assistant response).
  await expect(page.getByTestId("block-tool")).toHaveCount(2, { timeout: 5000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing…", { timeout: 2000 });

  // Fast tool finishes ~0.1s, slow at ~1.5s. Between those, status must
  // remain Pausing… — turn_paused only fires at the post-all-tools seam.
  await page.waitForTimeout(500);
  await expect(page.getByTestId("status-label")).toHaveText("Pausing…");

  // After the slow tool completes, seam fires.
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });
});

// 3.
test("pause + browser reload + manual continue: interjection lands on next LLM call", async ({ page }) => {
  await loadScript(SCRIPTS.multiTool());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("MULTI_TOOL_TEST reload test");
  await page.locator("textarea").press("Enter");

  await expect(page.getByTestId("block-tool").first()).toBeVisible({ timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 6000 });

  await page.reload();
  await waitForAlive(page);
  // turnState survives the reload — server re-broadcasts session_info.
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await expect(page.getByTestId("continue-btn")).toBeVisible();

  await page.locator("textarea").fill("simpler please");
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });

  const calls = await getCalls();
  expect(
    calls.some(
      c =>
        c.systemKind === "task" &&
        c.messages.some(m => m.role === "user" && m.content.includes("simpler please")),
    ),
  ).toBe(true);
});

// 4.
test("pre-commit + browser reload: pre-commit drops, paused turnState survives", async ({ page }) => {
  await loadScript(SCRIPTS.multiTool());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("MULTI_TOOL_TEST pre-commit");
  await page.locator("textarea").press("Enter");

  await expect(page.getByTestId("block-tool").first()).toBeVisible({ timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  // While in pause_requested (still running, before seam), click Continue
  // → arms pre-commit.
  await expect(page.getByTestId("status-label")).toHaveText("Pausing…", { timeout: 2000 });
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing, will continue");
  await expect(page.getByTestId("takeitback-btn")).toBeVisible();

  // Reload BEFORE the seam fires — the pre-commit (client-only) must be lost.
  await page.reload();
  await waitForAlive(page);

  // After reload the client state is fresh — no takeitback-btn.
  await expect(page.getByTestId("takeitback-btn")).not.toBeVisible({ timeout: 5000 });
  // Status reflects current server turnState (Pausing… or Paused by now).
  const label = await page.getByTestId("status-label").textContent();
  expect(["Pausing…", "Paused"]).toContain(label);

  // Drive to turn_end manually — since pre-commit was dropped, the client
  // shows the regular Continue button (from Paused state).
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });
});

// 5.
test("two pauses in one turn: both interjections appear in the feed and in later LLM calls", async ({ page }) => {
  await loadScript(SCRIPTS.twoPauses());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("TWO_PAUSES_TEST begin");
  await page.locator("textarea").press("Enter");

  // First pause cycle.
  await expect(page.getByTestId("block-tool").first()).toBeVisible({ timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await page.locator("textarea").fill("first interjection");
  await page.getByTestId("continue-btn").click();

  // Wait for the 2nd tool block (means the next LLM call returned and
  // another tool is running).
  await expect(page.getByTestId("block-tool")).toHaveCount(2, { timeout: 10000 });

  // Second pause cycle.
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await page.locator("textarea").fill("second interjection");
  await page.getByTestId("continue-btn").click();

  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 15000 });

  // Both interjections render as user blocks in the feed.
  const userTexts = await page.getByTestId("block-user").allInnerTexts();
  expect(userTexts.some(t => t.includes("first interjection"))).toBe(true);
  expect(userTexts.some(t => t.includes("second interjection"))).toBe(true);

  // Both interjections surface in subsequent LLM calls.
  const calls = await getCalls();
  expect(
    calls.some(c =>
      c.messages.some(m => m.role === "user" && m.content.includes("first interjection")),
    ),
  ).toBe(true);
  expect(
    calls.some(c =>
      c.messages.some(m => m.role === "user" && m.content.includes("second interjection")),
    ),
  ).toBe(true);
});

// 6.
test("pause during LLM stream does not truncate the assistant message", async ({ page }) => {
  await loadScript(SCRIPTS.longStream());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  await page.locator("textarea").fill("LONG_STREAM_TEST writeup");
  await page.locator("textarea").press("Enter");

  // Stream starts — status shows Streaming… before the full text arrives.
  await expect(page.getByTestId("status-label")).toHaveText("Streaming…", { timeout: 3000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Pausing…", { timeout: 2000 });

  // Pure-text response has no tool seam; the stream must finish and turn_end
  // lands, returning to Ready. The final assistant message must be complete.
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 5000 });

  const llmResponse = page.getByTestId("block-llm-response").last();
  await expect(llmResponse).toContainText("done stream");
  await expect(llmResponse).toContainText("This is a deliberately long streaming response");
});

// 7.
test("session resume basis includes interjection as 'User (mid-turn): ...'", async ({ page }) => {
  await loadScript(SCRIPTS.resumeBasis());
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  // Capture the session dir so we can find + Resume it later. The button's
  // `data-session-dir` holds the full path; the picker list renders only the
  // basename, so compare by basename.
  const originalSessionDirFull = await page
    .getByTestId("sessions-btn")
    .getAttribute("data-session-dir");
  expect(originalSessionDirFull).toBeTruthy();
  const originalDir = (originalSessionDirFull ?? "").split("/").pop() ?? "";

  await page.locator("textarea").fill("RESUME_BASIS_TEST begin");
  await page.locator("textarea").press("Enter");
  await expect(page.getByTestId("block-tool").first()).toBeVisible({ timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });
  await page.locator("textarea").fill("mid turn note");
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });

  // Reset call history so we can isolate the resumption LLM call.
  await resetCalls();

  // Open picker and start a fresh session, so the old one becomes resumable
  // (Resume is hidden on the currently-active session).
  await page.getByTestId("sessions-btn").click();
  await expect(page.getByTestId("session-picker-modal")).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("session-picker-new")).toBeEnabled({ timeout: 5000 });
  await page.getByTestId("session-picker-new").click();
  await expect(page.getByTestId("session-picker-modal")).not.toBeVisible({ timeout: 5000 });
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 5000 });

  // Reopen picker and Resume the original session.
  await page.getByTestId("sessions-btn").click();
  await expect(page.getByTestId("session-picker-modal")).toBeVisible({ timeout: 3000 });
  // Wait for the session list to load (replaces "Loading sessions…").
  await expect(page.getByTestId("session-picker-list")).toBeVisible({ timeout: 5000 });
  const items = page.getByTestId("session-picker-item");
  await expect(items.first()).toBeVisible({ timeout: 5000 });
  const count = await items.count();
  let resumed = false;
  for (let i = 0; i < count; i++) {
    const row = items.nth(i);
    const dir = (await row.locator(".session-picker-dir").textContent())?.trim();
    if (dir === originalDir) {
      await row.getByTestId("session-picker-resume").click();
      resumed = true;
      break;
    }
  }
  expect(resumed).toBe(true);

  // Wait for the resumption LLM call to complete. The resumption call is the
  // first task we can observe via the control API.
  await expect
    .poll(
      async () => {
        const calls = await getCalls();
        return calls.filter(c => c.systemKind === "resumption").length;
      },
      { timeout: 15000 },
    )
    .toBeGreaterThanOrEqual(1);

  const calls = await getCalls();
  const resumptionCalls = calls.filter(c => c.systemKind === "resumption");
  const firstResumption = resumptionCalls[0]!;
  const userMsg = firstResumption.messages.find(m => m.role === "user");
  expect(userMsg).toBeTruthy();
  expect(userMsg!.content).toContain("User (mid-turn): mid turn note");
});

// 8.
test("reload while Paused: no transient ⊘ Interrupted block ever renders", async ({ page }) => {
  // Observed in the wild (session 2026-04-20T06-29-36): refreshing the
  // browser while the server was in Paused briefly rendered a
  // `turn_interrupted` block in the event feed, which vanished on a
  // subsequent refresh. Test 3 above already reloads during Paused and
  // asserts the final status label, but it does not catch a transient
  // block-interrupt that appears for one frame and is then reconciled
  // away. This test installs a continuous DOM watcher so even a single
  // frame with the block is recorded.
  //
  // Watcher is installed via addInitScript so it is attached *before* the
  // bundle's first render on every navigation (the initial goto and the
  // reload). It combines a MutationObserver (fires on any DOM mutation)
  // with a requestAnimationFrame poll (fires every frame) to catch both
  // mutation- and rerender-driven appearances.
  await page.addInitScript(() => {
    // Reset on every navigation so the reload's observations start clean.
    (window as unknown as { __interruptSightings: number[] }).__interruptSightings = [];
    const record = () => {
      if (document.querySelector('[data-testid="block-interrupt"]')) {
        (window as unknown as { __interruptSightings: number[] }).__interruptSightings
          .push(Date.now());
      }
    };
    const start = () => {
      record();
      new MutationObserver(record).observe(document.documentElement, {
        childList: true,
        subtree: true,
      });
      const loop = () => { record(); requestAnimationFrame(loop); };
      requestAnimationFrame(loop);
    };
    if (document.documentElement) start();
    else document.addEventListener("readystatechange", start, { once: true });
  });

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await openNewSession(page);

  // Trigger a pausable multi-tool turn (same scenario as test 3).
  await loadScript(SCRIPTS.multiTool());
  await page.locator("textarea").fill("MULTI_TOOL_TEST flash probe");
  await page.locator("textarea").press("Enter");

  await expect(page.getByTestId("block-tool").first()).toBeVisible({ timeout: 10000 });
  await page.getByTestId("pause-btn").click();
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 6000 });

  // Clear any pre-reload sightings (there should be none; this is defensive)
  // so the assertion below is strictly about the post-reload window.
  await page.evaluate(() => {
    (window as unknown as { __interruptSightings: number[] }).__interruptSightings = [];
  });

  await page.reload();
  await waitForAlive(page);
  await expect(page.getByTestId("status-label")).toHaveText("Paused", { timeout: 5000 });

  // Give the UI a settling window to expose any flash the reconnect flow
  // might produce. 1 s is plenty: session_info + history + ready round-trip
  // in tens of ms on localhost.
  await page.waitForTimeout(1000);

  const sightings = await page.evaluate(
    () => (window as unknown as { __interruptSightings: number[] }).__interruptSightings,
  );
  expect(sightings, `block-interrupt was rendered ${sightings.length} time(s) after reload`).toEqual([]);

  // Drive the turn cleanly to completion so the test leaves no zombie.
  await page.getByTestId("continue-btn").click();
  await expect(page.getByTestId("block-turn-end").last()).toBeVisible({ timeout: 10000 });
});
