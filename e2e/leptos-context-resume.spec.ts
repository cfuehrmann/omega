/**
 * Phase 3.5 — Leptos context inspector + resume e2e tests.
 *
 * Drives the modal + resume flows mounted at `/leptos/` against
 * `mock-omega-server` (port 3003, real-server project). Four
 * concerns under test:
 *
 * 1. **Open modal on llm_call** — clicking the `[context]` button on
 *    an `llm_call` block opens the full-viewport modal, fires
 *    `GET /api/context?hashes=…`, and renders one
 *    `[data-testid="leptos-context-modal-record"]` per returned
 *    record with its role + body.
 *
 * 2. **Modal dismissal via close button** — clicking
 *    `[data-testid="leptos-context-modal-close"]` closes the modal
 *    (the backdrop + content vanish from the DOM via the `<Show>`
 *    wrapper). Click-outside / Esc dismissal are JS-interop gaps —
 *    not exercised here, mirroring the same pattern as 3.1–3.4.
 *
 * 3. **Payload modal on llm_call** (TODO-B, Phase 3.10) — clicking
 *    the `[payload]` button (`leptos-llm-call-payload`) opens the
 *    `TextModal` overlay. The modal body contains the four
 *    metadata fields (`cache_breakpoint_index`, `request_bytes`,
 *    the context hashes list, and `request_summary`). Closing via
 *    `leptos-text-modal-close` hides the modal.
 *
 * 4. **Resume from picker triggers resumption summary turn** —
 *    drives `SCRIPTS.resumeBasis()` which feeds the mock
 *    `/v1/messages` server a tool turn + a final text + a
 *    synthetic resumption summary. Clicking the source row's
 *    `[data-testid="leptos-session-resume"]` button sends
 *    `ClientFrame::ResumeSession`; the server emits
 *    `session_info → history → resuming_session → session_resumed
 *    → ready` for the new session, and the feed renders both the
 *    `resuming_session` and `session_resumed` events.
 *
 * Lifespan: deleted in Phase 3.7 alongside the rest of Playwright
 * when chromiumoxide takes over.
 */

import { test, expect, type Page } from "@playwright/test";
import { SCRIPTS, loadScript, resetCalls } from "./fixtures/real-server-control";

/**
 * Wait for the WS to connect, then navigate to the Leptos UI.
 * Uses `data-connected` on <main> instead of the debug-panel snapshot
 * (debug panel is cfg(debug_assertions)-only — Phase 3.9 TODO-4).
 */
async function gotoLeptos(page: Page) {
  await page.goto("/leptos/");
  await expect(page.locator('main[data-connected="true"]'))
    .toBeAttached({ timeout: 5000 });
}

/** Send a user message via the production composer (Phase 3.4). */
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
 * Click `+ new session` and wait for the new dir to land.
 * NOTE: auto-closes the picker (Phase 3.9 TODO-2); open via
 * `leptos-composer-sessions` before accessing picker rows again.
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
// 1 + 2: Modal open on llm_call → renders ContextRecords → close dismisses
// ---------------------------------------------------------------------------

test("leptos-context: clicking llm_call opens modal, fetches records, close dismisses", async ({ page }) => {
  await resetCalls();
  // Single-tool turn keeps the test deterministic and ensures at
  // least one `llm_call` lands in the feed.
  await loadScript([
    { kind: "toolUse", id: "toolu_ctx_1", name: "run_command", input: { command: "echo ctx" } },
    { kind: "text", text: "done ctx" },
  ]);
  await gotoLeptos(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "trigger llm_call");

  // Wait for turn_end so we have at least one persisted llm_call
  // block in the feed.
  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 15000 });

  const llmCalls = feed.locator('[data-event-type="llm_call"]');
  expect(await llmCalls.count()).toBeGreaterThanOrEqual(1);

  // Modal closed initially — the <Show> wrapper means the backdrop
  // is not in the DOM at all.
  await expect(page.getByTestId("leptos-context-modal")).toHaveCount(0);

  // Click the first llm_call's "[context]" button.
  await llmCalls.first().getByTestId("leptos-llm-call-open-modal").click();

  // Modal becomes visible.
  const modal = page.getByTestId("leptos-context-modal");
  await expect(modal).toBeVisible({ timeout: 3000 });

  // Loading state lands eventually one of two outcomes: records or
  // the "no context records returned" fallback. Either way, the
  // loading spinner clears.
  await expect(page.getByTestId("leptos-context-modal-loading"))
    .toBeHidden({ timeout: 5000 });

  // The fetch returned at least one record (the user's message).
  // We assert via the records list rather than counting against a
  // hardcoded expected value because real session bookkeeping varies.
  const records = page.getByTestId("leptos-context-modal-record");
  expect(await records.count()).toBeGreaterThanOrEqual(1);

  // Each record has a role attribute and a body element.
  const firstRec = records.first();
  await expect(firstRec).toHaveAttribute("data-role", /(user|assistant)/);
  await expect(firstRec.getByTestId("leptos-context-modal-record-body"))
    .toBeVisible();

  // The meta line includes the hash count and request bytes.
  await expect(page.getByTestId("leptos-context-modal-meta"))
    .toContainText(/\d+ hash\(es\) · \d+ bytes/);

  // Close button dismisses the modal — the <Show> wrapper means
  // the entire backdrop disappears.
  await page.getByTestId("leptos-context-modal-close").click();
  await expect(page.getByTestId("leptos-context-modal")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 3: Payload modal reveals llm_call metadata (TODO-B, Phase 3.10)
// ---------------------------------------------------------------------------

test("leptos-context: llm_call [payload] button opens TextModal with metadata fields", async ({ page }) => {
  await resetCalls();
  await loadScript([
    { kind: "text", text: "ping" },
  ]);
  await gotoLeptos(page);

  const startDir = await readActiveDir(page);
  await newSession(page, startDir);

  await sendComposerMessage(page, "trigger payload modal");

  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 15000 });

  const llmCall = feed.locator('[data-event-type="llm_call"]').first();

  // The <details> expander is gone (TODO-B). In its place, a [payload]
  // button opens the TextModal overlay.
  await expect(llmCall.getByTestId("leptos-llm-call-details")).toHaveCount(0);
  await expect(llmCall.getByTestId("leptos-llm-call-payload")).toBeVisible();

  // Modal is closed initially.
  await expect(page.getByTestId("leptos-text-modal")).toHaveCount(0);

  // Click [payload].
  await llmCall.getByTestId("leptos-llm-call-payload").click();

  // TextModal appears.
  const modal = page.getByTestId("leptos-text-modal");
  await expect(modal).toBeVisible({ timeout: 3000 });

  // Title indicates this is an llm_call payload.
  await expect(page.getByTestId("leptos-text-modal-title"))
    .toContainText("llm_call payload");

  // Modal body contains all four metadata fields.
  const body = page.getByTestId("leptos-text-modal-body");
  await expect(body).toContainText("model:");
  await expect(body).toContainText("cache_breakpoint_index:");
  await expect(body).toContainText("request_bytes:");
  await expect(body).toContainText("context_hashes:");
  await expect(body).toContainText("request_summary");

  // request_bytes value is a positive integer.
  const bodyText = await body.innerText();
  const bytesMatch = bodyText.match(/request_bytes:\s*(\d+)/);
  expect(bytesMatch).not.toBeNull();
  expect(parseInt(bytesMatch![1], 10)).toBeGreaterThan(0);

  // Close via ✕ button.
  await page.getByTestId("leptos-text-modal-close").click();
  await expect(page.getByTestId("leptos-text-modal")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 4: Resume from picker triggers resumption summary turn
// ---------------------------------------------------------------------------

test("leptos-context: resume button from picker drives full resumption flow", async ({ page }) => {
  await resetCalls();
  // resumeBasis has three steps total: tool turn, final text, then
  // a synthetic summary that the resumption pass consumes.
  await loadScript(SCRIPTS.resumeBasis());
  await gotoLeptos(page);

  // Create a source session, run one turn so it has assistant
  // history (required for resumption-basis extraction).
  const startDir = await readActiveDir(page);
  const sourceDir = await newSession(page, startDir);
  await sendComposerMessage(page, "seed the source session");

  // Wait for the seed turn to finish.
  const feed = page.getByTestId("leptos-feed");
  await expect(
    feed.locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 15000 });

  // The picker now has a row for sourceDir; open the picker and
  // locate the resume button on that specific row.
  // (picker was auto-closed by newSession — Phase 3.9 TODO-2)
  await page.getByTestId("leptos-composer-sessions").click();
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible({ timeout: 2000 });
  const sourceRow = page.locator(
    `[data-testid="leptos-session-item"][data-session-dir="${sourceDir}"]`
  );
  await expect(sourceRow).toBeVisible();
  await expect(sourceRow.getByTestId("leptos-session-resume")).toBeVisible();

  // Click resume — the server should:
  //   1. Replace the active session with a fresh one (new dir).
  //   2. Run a resumption-basis LLM call against the source dir.
  //   3. Emit `resuming_session` (forwarded as data-event-type).
  //   4. Emit `session_resumed` once the basis has been written.
  await sourceRow.getByTestId("leptos-session-resume").click();

  // The active session changes to a new dir.
  await expect.poll(async () => {
    const dir = await readActiveDir(page);
    return dir !== sourceDir && dir !== null;
  }, { timeout: 10000 }).toBeTruthy();

  // The feed renders a `resuming_session` block referencing the
  // source dir. The 3.3 `<EventBlock/>` projects this as the
  // status family with `data-event-type="resuming_session"`.
  await expect(
    feed.locator('[data-event-type="resuming_session"]')
  ).toHaveCount(1, { timeout: 15000 });
  await expect(
    feed.locator('[data-event-type="resuming_session"]').first()
  ).toContainText(sourceDir);

  // The feed renders a `session_resumed` block with the synthetic
  // summary the mock provider supplied via SCRIPTS.resumeBasis().
  await expect(
    feed.locator('[data-event-type="session_resumed"]')
  ).toHaveCount(1, { timeout: 15000 });
  await expect(
    feed.locator('[data-event-type="session_resumed"]').first()
  ).toContainText("Resumed session summary");
});
