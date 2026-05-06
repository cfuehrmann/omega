/**
 * Phase 3.3 — Leptos conversation feed e2e tests.
 *
 * Drives the feed mounted at `/leptos/` against `mock-omega-server`
 * (port 3003 in the real-server project). Three concerns under test:
 *
 * 1. Multi-tool turn — every visible event family renders. The
 *    `multiTool` script (already in real-server-control SCRIPTS) walks
 *    the agent through three sequential `run_command` tool turns then
 *    a final `text`. We assert that every variant in the resulting
 *    feed has a `data-event-type` attribute matching the wire
 *    discriminator, and that the six `data-event-kind` families
 *    (user / assistant / tool_call / tool_result / status / error)
 *    each appear at least once. (`error` is exercised by the second
 *    test — see `httpError400` below.)
 *
 * 2. Streaming text — the `longStream` script chunks a single text
 *    response over 8 frames with a 100 ms delay each. The streaming
 *    overlay must appear during the turn and the final `llm_response`
 *    must show the assembled text after `turn_end`.
 *
 * 3. Tool-result truncation (TODO-C, Phase 3.10) — drive a `read_file`
 *    against rust-migration.md (long enough to span many lines) and
 *    assert the inline preview is bounded to 2 lines, the old
 *    `[show more]` toggle is gone, and the `[payload]` button opens
 *    a `TextModal` containing the full content.
 *
 * The fourth test exercises the Error family via a `httpError(400)`
 * script step — terminal LLM error — which surfaces as
 * `data-event-type="llm_error"` with `data-event-kind="error"`.
 *
 * All four specs use the production composer (`leptos-composer-*`)
 * — which replaced the 3.3 stub composer in 3.4 — to send
 * `user_message` frames over the WS.
 *
 * Lifespan: deleted in Phase 3.7 alongside the rest of Playwright when
 * chromiumoxide takes over.
 */

import { test, expect, type Page } from "@playwright/test";
import { SCRIPTS, loadScript, resetCalls } from "./fixtures/real-server-control";

/**
 * Wait for the WS to connect, then navigate to the Leptos UI.
 */
async function gotoFeed(page: Page) {
  await page.goto("/leptos/");
  // Wait for WS connection via `data-connected` on <main>.
  // The debug panel is cfg(debug_assertions)-only (Phase 3.9 TODO-4).
  await expect(page.locator('main[data-connected="true"]'))
    .toBeAttached({ timeout: 5000 });
}

/**
 * Send a user message via the production composer (Phase 3.4).
 * Types into the textarea then presses Enter — mirrors the SolidJS
 * UI's keyboard semantics.
 */
async function sendComposerMessage(page: Page, content: string) {
  const input = page.getByTestId("leptos-composer-input");
  await input.fill(content);
  await input.press("Enter");
}

/** Read the active session dir from `data-active-session-dir` on <main>. */
async function readActiveDir(page: Page): Promise<string | null> {
  const val = await page.locator("main").getAttribute("data-active-session-dir");
  return val || null;
}

/**
 * Click `+ new session` and wait for the new session_info.dir to land.
 * NOTE: auto-closes the picker (Phase 3.9 TODO-2); open it again
 * before accessing picker rows.
 */
async function newSession(page: Page, prev: string | null): Promise<string> {
  // Phase 3.10 TODO-F: ensure the picker is open before clicking + new.
  if ((await page.getByTestId("leptos-session-picker").count()) === 0) {
    await page.getByTestId("leptos-composer-sessions").click();
  }
  await page.getByTestId("leptos-session-new").click();
  let next: string | null = null;
  await expect.poll(async () => {
    next = await readActiveDir(page);
    return next !== null && next !== prev;
  }, { timeout: 5000 }).toBeTruthy();
  return next as unknown as string;
}

// ---------------------------------------------------------------------------
// 1. Multi-tool turn — every visible event family renders
// ---------------------------------------------------------------------------

test("leptos-feed: multi-tool turn renders every visible event family", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.multiTool());
  await gotoFeed(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "go multi tool");

  // The multi-tool script ends with a plain text turn after three
  // tool_call/tool_result pairs — `turn_end` is the synchronization
  // point. Allow plenty of time because each `sleep 0.6` runs serially.
  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-testid="leptos-event-block"][data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 30000 });

  // user_message renders.
  await expect(
    feed.locator('[data-event-type="user_message"]')
  ).toHaveCount(1);
  await expect(
    feed.locator('[data-event-type="user_message"] [data-testid="leptos-user-content"]')
  ).toContainText("go multi tool");

  // Every tool_call has a typed body (name + JSON input).
  const toolCalls = feed.locator('[data-event-type="tool_call"]');
  await expect(toolCalls).toHaveCount(3);
  await expect(toolCalls.first().locator('[data-testid="leptos-tool-name"]'))
    .toHaveText("run_command");
  await expect(toolCalls.first().locator('[data-testid="leptos-tool-input"]'))
    .toContainText("sleep 0.6");

  // Every tool_result has a body, none flagged as errors here.
  const toolResults = feed.locator('[data-event-type="tool_result"]');
  await expect(toolResults).toHaveCount(3);
  for (let i = 0; i < 3; i++) {
    const r = toolResults.nth(i);
    await expect(r).toHaveAttribute("data-event-kind", "tool_result");
  }

  // llm_response (assistant family) present at least once with the
  // final text.
  const responses = feed.locator('[data-event-type="llm_response"]');
  await expect(responses.last()).toHaveAttribute("data-event-kind", "assistant");
  await expect(responses.last().locator('[data-testid="leptos-assistant-text"]'))
    .toContainText("done multi");

  // Status family represented (turn_end / llm_call / session_started /
  // server_started — at least one).
  await expect(feed.locator('[data-event-kind="status"]').first())
    .toBeVisible();

  // Sanity: every event block has both a kind and a type.
  const totalBlocks = await feed.locator('[data-testid="leptos-event-block"]').count();
  expect(totalBlocks).toBeGreaterThan(5);
  const blocksWithoutKind = await feed
    .locator('[data-testid="leptos-event-block"]:not([data-event-kind])')
    .count();
  expect(blocksWithoutKind).toBe(0);
  const blocksWithoutType = await feed
    .locator('[data-testid="leptos-event-block"]:not([data-event-type])')
    .count();
  expect(blocksWithoutType).toBe(0);
});

// ---------------------------------------------------------------------------
// 2. Streaming text appears live during a long-stream turn
// ---------------------------------------------------------------------------

test("leptos-feed: streaming text overlay appears live and resolves into llm_response", async ({ page }) => {
  await resetCalls();
  await loadScript(SCRIPTS.longStream());
  await gotoFeed(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "go long stream");

  // While the SSE stream is still emitting chunks, the overlay must
  // be visible and grow. The longStream script is 8 chunks × 100 ms
  // = ~800 ms of streaming, plenty of window to observe.
  const overlay = page.getByTestId("leptos-streaming-text");
  await expect(overlay).toBeVisible({ timeout: 5000 });
  await expect(overlay).toContainText("This is a deliberately long");

  // After turn_end, the overlay clears (SessionStore::apply resets
  // streaming_text on TurnEnd) and the full text lives inside the
  // persisted llm_response block.
  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 10000 });
  await expect(overlay).toBeHidden();
  await expect(
    feed
      .locator('[data-event-type="llm_response"]')
      .last()
      .locator('[data-testid="leptos-assistant-text"]')
  ).toContainText("done stream");
});

// ---------------------------------------------------------------------------
// 3. Tool-result truncation — [payload] modal reveals full body (TODO-C)
// ---------------------------------------------------------------------------

test("leptos-feed: long tool_result shows 2-line preview; [payload] button opens TextModal with full content", async ({ page }) => {
  await resetCalls();
  // Drive a single read_file tool turn against rust-migration.md, which
  // is several KB long — many lines — so the 2-line preview must truncate.
  await loadScript([
    {
      kind: "toolUse",
      id: "toolu_long_read",
      name: "read_file",
      input: { path: "rust-migration.md" },
    },
    { kind: "text", text: "done long read" },
  ]);
  await gotoFeed(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "trigger long read");

  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 15000 });

  const result = feed.locator('[data-event-type="tool_result"]');
  await expect(result).toHaveCount(1);

  // The [show more] toggle is gone (TODO-C). Only the [payload] button exists.
  await expect(result.getByTestId("leptos-tool-result-expand")).toHaveCount(0);
  await expect(result.getByTestId("leptos-tool-result-payload")).toBeVisible();

  // The inline preview is bounded to the first 2 lines (no 3000-char
  // truncation marker any more). The body must NOT contain the old marker.
  const body = result.locator('[data-testid="leptos-tool-result-body"]');
  await expect(body).not.toContainText("chars total — showing first");

  // Inline preview text is short (2 lines means well under 500 chars for
  // any reasonable file line length).
  const previewText = await body.innerText();
  const previewLines = previewText.split("\n").filter((l: string) => l.length > 0);
  expect(previewLines.length).toBeLessThanOrEqual(2);

  // [payload] opens the TextModal with the full file content.
  await expect(page.getByTestId("leptos-text-modal")).toHaveCount(0);
  await result.getByTestId("leptos-tool-result-payload").click();

  const modal = page.getByTestId("leptos-text-modal");
  await expect(modal).toBeVisible({ timeout: 3000 });

  // Title contains the tool name and duration.
  await expect(page.getByTestId("leptos-text-modal-title"))
    .toContainText("read_file");

  // Full body is substantially longer than the 2-line preview.
  const modalBody = page.getByTestId("leptos-text-modal-body");
  const fullText = await modalBody.innerText();
  expect(fullText.length).toBeGreaterThan(previewText.length + 100);

  // Close the modal.
  await page.getByTestId("leptos-text-modal-close").click();
  await expect(page.getByTestId("leptos-text-modal")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 4. Error family — terminal LLM error renders with kind=error
// ---------------------------------------------------------------------------

test("leptos-feed: terminal llm_error renders in the error family", async ({ page }) => {
  await resetCalls();
  await loadScript([
    {
      kind: "httpError",
      status: 400,
      body: '{"type":"error","error":{"type":"invalid_request_error","message":"bad input"}}',
    },
  ]);
  await gotoFeed(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "trigger 400");

  const feed = page.getByTestId("leptos-feed");
  // The agent will emit a `turn_interrupted` (kind=error) when the
  // turn fails, plus an `llm_error` (kind=error) carrying the HTTP
  // status. Either one provides Error-family coverage.
  const errorBlocks = feed.locator('[data-event-kind="error"]');
  await expect(errorBlocks.first()).toBeVisible({ timeout: 15000 });

  // At least one of: llm_error, turn_interrupted (both kind=error).
  const llmError = feed.locator('[data-event-type="llm_error"]');
  const turnInterrupted = feed.locator('[data-event-type="turn_interrupted"]');
  const errorTypeCount =
    (await llmError.count()) + (await turnInterrupted.count());
  expect(errorTypeCount).toBeGreaterThan(0);
});
