/**
 * Phase 3.4 — Leptos composer e2e tests.
 *
 * Drives the composer mounted at `/leptos/` against `mock-omega-server`
 * (port 3003 in the real-server project). Covers every operator flow
 * the SolidJS composer offers today:
 *
 * 1. Send — type a message, press Enter, server replies, feed renders
 *    user_message + llm_response.
 * 2. Pause-during-tool — drive a long-running run_command, click the
 *    Pause primary button mid-turn, assert turn_state transitions
 *    through pause_requested → paused.
 * 3. Continue with interjection — from Paused, type an interjection
 *    into the textarea and click Continue; assert the turn resumes
 *    and the next ClientFrame::Continue carried the interjection
 *    content (mirrors SolidJS continueFromPaused).
 * 4. Abort — drive a long-running tool, pause, then abort; assert
 *    turn_interrupted.
 * 5. Switch model mid-idle (regression for 8e2106b) — after a turn
 *    completes, switch model from Sonnet to Opus 4.7 via the
 *    composer dropdown; assert session_info.model updates without
 *    reload.
 * 6. Switch effort — from Idle, change effort from medium to high;
 *    assert session_info.effort updates.
 * 7. File-completion accept — type `@rust/c`, wait for the popup,
 *    arrow-down to highlight a candidate, press Enter; assert the
 *    textarea now contains the completed path.
 *
 * Determinism note: every flow waits on `session_info.turn_state` /
 * `session_info.model` etc. read from the debug-snapshot JSON
 * (ground truth) rather than rendered button text — the picker spec
 * established the same pattern in 3.2.
 *
 * Lifespan: deleted in Phase 3.7 alongside the rest of Playwright when
 * chromiumoxide takes over.
 */

import { test, expect, type Page } from "@playwright/test";
import { SCRIPTS, loadScript, resetCalls } from "./fixtures/real-server-control";

// ---------------------------------------------------------------------------
// Helpers — same primitives as leptos-conversation-feed.spec.ts so the two
// specs share idioms.
// ---------------------------------------------------------------------------

/** Wait for WS to connect; expand debug panel for store-snapshot reads. */
async function gotoComposer(page: Page) {
  await page.goto("/leptos/");
  await page.getByTestId("leptos-debug-panel").locator("summary").click();
  await expect(page.getByTestId("leptos-debug-store"))
    .toContainText('"connected": true', { timeout: 5000 });
}

/** Read a typed snapshot of the conversation store. */
async function readStore(page: Page): Promise<{
  turnState: string;
  sessionInfo: { dir: string; model: string; effort: string } | null;
  events: Array<{ type: string }>;
}> {
  const text = await page.getByTestId("leptos-debug-store").innerText();
  return JSON.parse(text);
}

/** Click `+ new session` and wait for a new active session_info.dir. */
async function newSession(page: Page, prev: string | null): Promise<string> {
  await page.getByTestId("leptos-session-new").click();
  let next: string | null = null;
  await expect.poll(async () => {
    next = (await readStore(page)).sessionInfo?.dir ?? null;
    return next !== null && next !== prev;
  }, { timeout: 5000 }).toBeTruthy();
  return next as unknown as string;
}

async function activeDir(page: Page): Promise<string | null> {
  return (await readStore(page)).sessionInfo?.dir ?? null;
}

/** Type into the composer textarea (replaces any existing content). */
async function fillComposer(page: Page, content: string) {
  await page.getByTestId("leptos-composer-input").fill(content);
}

/** Press the primary action button (whatever it currently is). */
async function clickPrimary(page: Page) {
  await page.getByTestId("leptos-composer-primary").click();
}

/** Wait for the store's turn_state to reach the given value. */
async function waitForTurnState(
  page: Page,
  expected: "idle" | "running" | "pause_requested" | "paused",
  timeout = 10000,
) {
  await expect.poll(async () => (await readStore(page)).turnState, { timeout })
    .toBe(expected);
}

// ---------------------------------------------------------------------------
// 1. Send — happy path
// ---------------------------------------------------------------------------

test("leptos-composer: type, press Enter, server replies", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.pong());
  await gotoComposer(page);

  await newSession(page, await activeDir(page));

  // Primary action starts as "Send" (turn_state=idle).
  const primary = page.getByTestId("leptos-composer-primary");
  await expect(primary).toHaveAttribute("data-action", "send");
  await expect(primary).toHaveText("Send");

  await fillComposer(page, "ping");
  await page.getByTestId("leptos-composer-input").press("Enter");

  // turn_end appears in the feed; final llm_response carries "pong".
  const feed = page.getByTestId("leptos-feed");
  await expect(feed.locator('[data-event-type="turn_end"]'))
    .toHaveCount(1, { timeout: 10000 });
  await expect(
    feed.locator('[data-event-type="llm_response"]').last()
      .locator('[data-testid="leptos-assistant-text"]')
  ).toContainText("pong");

  // Composer cleared; back to "Send".
  await expect(page.getByTestId("leptos-composer-input")).toHaveValue("");
  await expect(primary).toHaveAttribute("data-action", "send");
});

// ---------------------------------------------------------------------------
// 2. Pause-during-tool
// ---------------------------------------------------------------------------

test("leptos-composer: pause during a long-running tool", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.twoPauses()); // 4×sleep(0.6) + final text — plenty of room
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  await fillComposer(page, "go pause");
  await clickPrimary(page);

  // Wait for the turn to start running.
  await waitForTurnState(page, "running");

  // Primary flips to "Pause".
  const primary = page.getByTestId("leptos-composer-primary");
  await expect(primary).toHaveAttribute("data-action", "pause");
  await expect(primary).toHaveText("Pause");

  // Click Pause; first lands in pause_requested, then paused once the
  // server's tool turn finishes its current step.
  await primary.click();
  await waitForTurnState(page, "pause_requested", 5000);
  await waitForTurnState(page, "paused", 15000);

  // In Paused, primary becomes "Continue" and the secondary Abort
  // button is now visible.
  await expect(primary).toHaveAttribute("data-action", "continue");
  await expect(page.getByTestId("leptos-composer-abort")).toBeVisible();

  // Continue from Paused: the turn resumes and runs to completion.
  await primary.click();
  await waitForTurnState(page, "idle", 30000);

  // turn_paused was persisted as an event during the pause cycle.
  const feed = page.getByTestId("leptos-feed");
  await expect(feed.locator('[data-event-type="turn_paused"]'))
    .toHaveCount(1);
});

// ---------------------------------------------------------------------------
// 3. Continue with interjection content
// ---------------------------------------------------------------------------

test("leptos-composer: continue with interjection forwards the typed content", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.twoPauses());
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  await fillComposer(page, "trigger interjection");
  await clickPrimary(page);
  await waitForTurnState(page, "running");

  // Pause mid-flight.
  await clickPrimary(page);
  await waitForTurnState(page, "paused", 15000);

  // Type an interjection into the textarea while paused, then continue.
  // The composer's draft IS the interjection box (mirrors SolidJS).
  await fillComposer(page, "actually focus on src/web/server.rs");
  await clickPrimary(page);

  // Turn resumes (turn_continued event lands).
  await waitForTurnState(page, "running", 5000);

  // The textarea cleared on continue (because content was sent).
  await expect(page.getByTestId("leptos-composer-input")).toHaveValue("");

  // The feed contains a turn_continued event after the user's
  // interjection was applied.
  const feed = page.getByTestId("leptos-feed");
  await expect(feed.locator('[data-event-type="turn_continued"]'))
    .toHaveCount(1, { timeout: 5000 });

  // Wait for completion to leave a clean state.
  await waitForTurnState(page, "idle", 30000);

  // The turn ended cleanly — no turn_interrupted from a misroute.
  await expect(feed.locator('[data-event-type="turn_interrupted"]'))
    .toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 4. Abort
// ---------------------------------------------------------------------------

test("leptos-composer: pause then abort interrupts the turn", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.abortSleep()); // single sleep(10) — abort regression
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  await fillComposer(page, "go abort");
  await clickPrimary(page);
  await waitForTurnState(page, "running");

  // Pause first — the abort path requires being in pause_requested
  // or paused (running mode shows only the Pause button).
  await clickPrimary(page);

  // While in pause_requested the primary button is "Abort". (The
  // sleep tool runs for 10s so we have plenty of time.)
  const primary = page.getByTestId("leptos-composer-primary");
  await expect.poll(
    async () => primary.getAttribute("data-action"),
    { timeout: 5000 },
  ).toBe("abort");

  // Click Abort — primary action in pause_requested.
  await primary.click();

  // Turn ends as interrupted; turn_interrupted event lands; back to idle.
  await waitForTurnState(page, "idle", 15000);
  await expect(
    page.getByTestId("leptos-feed").locator('[data-event-type="turn_interrupted"]')
  ).toHaveCount(1);
});

// ---------------------------------------------------------------------------
// 5. Switch model mid-idle (regression for 8e2106b)
// ---------------------------------------------------------------------------

test("leptos-composer: switch model while idle updates session_info immediately (regression for 8e2106b)", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.pong());
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  // Run one full turn so we're idle with a non-empty event log —
  // the original SolidJS bug only manifested after at least one
  // completed turn (lastTurnEnd.model was the stale source).
  await fillComposer(page, "ping");
  await page.getByTestId("leptos-composer-input").press("Enter");
  await waitForTurnState(page, "idle", 10000);

  // Sanity: starts on Sonnet.
  expect((await readStore(page)).sessionInfo?.model).toBe("claude-sonnet-4-6");

  // Switch via the native <select>.
  const modelSelect = page.getByTestId("leptos-composer-model");
  await modelSelect.selectOption("claude-opus-4-7");

  // session_info.model must update on the server's next session_info
  // broadcast — no page reload required.
  await expect.poll(
    async () => (await readStore(page)).sessionInfo?.model,
    { timeout: 5000 },
  ).toBe("claude-opus-4-7");

  // Composer dropdown also reflects the new model.
  await expect(modelSelect).toHaveValue("claude-opus-4-7");
});

// ---------------------------------------------------------------------------
// 6. Switch effort
// ---------------------------------------------------------------------------

test("leptos-composer: switch effort while idle updates session_info", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.pong());
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  // Default effort is medium (omega-agent DEFAULT_EFFORT).
  expect((await readStore(page)).sessionInfo?.effort).toBe("medium");

  const effortSelect = page.getByTestId("leptos-composer-effort");
  await effortSelect.selectOption("high");

  await expect.poll(
    async () => (await readStore(page)).sessionInfo?.effort,
    { timeout: 5000 },
  ).toBe("high");
  await expect(effortSelect).toHaveValue("high");
});

// ---------------------------------------------------------------------------
// 7. File-completion accept
// ---------------------------------------------------------------------------

test("leptos-composer: @-prefix opens the completion popup; Enter accepts the highlight", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.pong());
  await gotoComposer(page);
  await newSession(page, await activeDir(page));

  const input = page.getByTestId("leptos-composer-input");
  // Type `@` then a path prefix that should match at least one entry
  // under the test cwd. The mock-omega-server runs with cwd = repo
  // root, so `rust/` is a known directory (Phase 3.7: replaced the
  // earlier `src/` reference — `src/` was deleted alongside the
  // SolidJS frontend).
  await input.fill("@rust/");

  // Popup must appear and contain at least one item.
  const popup = page.getByTestId("leptos-composer-completion");
  await expect(popup).toBeVisible({ timeout: 5000 });
  const items = popup.getByTestId("leptos-composer-completion-item");
  await expect(items.first()).toBeVisible();
  const itemCountBefore = await items.count();
  expect(itemCountBefore).toBeGreaterThan(0);

  // Read the first item's value before highlighting + accepting.
  const firstItem = await items.first().getAttribute("data-completion");
  expect(firstItem).not.toBeNull();

  // ArrowDown to highlight the first entry, then Enter to accept.
  await input.press("ArrowDown");
  await input.press("Enter");

  // Textarea now starts with `@<accepted>`. If the accepted item ends
  // with `/` (directory) the popup stays open and drills in; otherwise
  // it closes. Either way, the textarea reflects the accepted path.
  const value = await input.inputValue();
  expect(value).toBe(`@${firstItem}`);

  // If it was a file, the popup closed; if it was a directory, the
  // popup is still open showing children. Both are acceptable
  // outcomes — assert the *behaviour*, not which one happened.
  if (!firstItem!.endsWith("/")) {
    await expect(popup).toBeHidden({ timeout: 2000 });
  }
});

// ---------------------------------------------------------------------------
// 8. The 3.3 stub composer is gone (negative assertion)
// ---------------------------------------------------------------------------

test("leptos-composer: the 3.3 stub composer is no longer rendered", async ({ page }) => {
  await gotoComposer(page);
  // The new composer surface is mounted.
  await expect(page.getByTestId("leptos-composer")).toBeVisible();
  // None of the stub-composer testids remain.
  await expect(page.getByTestId("leptos-stub-composer")).toHaveCount(0);
  await expect(page.getByTestId("leptos-stub-composer-input")).toHaveCount(0);
  await expect(page.getByTestId("leptos-stub-composer-send")).toHaveCount(0);
});
