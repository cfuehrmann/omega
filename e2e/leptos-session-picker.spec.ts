/**
 * Phase 3.2 — Leptos session picker e2e tests.
 *
 * Drives the picker mounted at `/leptos/` against `mock-omega-server`
 * (port 3003 in the real-server project). Covers all four CRUD
 * operations:
 *
 * 1. Create — `+ new session` button sends `ClientFrame::Reset` over
 *    the open WebSocket. The server emits `session_info → reset_done →
 *    history → ready`; the picker's `Effect` watching
 *    `session_info.dir` refetches `/api/sessions`; the new dir appears
 *    in the list and is marked active.
 * 2. Rename — inline rename on a row sends `ClientFrame::RenameSession`.
 *    The server broadcasts `session_renamed`; the `SessionListStore`
 *    reducer updates the matching item's `name` field; the row's
 *    label re-renders.
 * 3. Delete — accepting the `window.confirm` prompt sends
 *    `ClientFrame::DeleteSession`. The server broadcasts
 *    `session_deleted`; the reducer removes the entry.
 * 4. List — initial render fetches `/api/sessions` and shows the
 *    server-side list; refresh on `session_info.dir` change keeps
 *    it in sync after Reset.
 *
 * ## Determinism note
 *
 * The picker's `data-active="true"` attribute can briefly point at the
 * *previous* active row between when `+ new session` is clicked and
 * when the server's `session_info` for the new session arrives. So we
 * never read `data-session-dir` straight off the active row right
 * after a click — instead we read the conversation store's
 * `session_info.dir` from the debug-snapshot JSON (ground truth) and
 * wait for the picker list to reflect that dir.
 *
 * Pre-existing sessions in `.omega/test-sessions/` (left by other
 * specs) are tolerated — every assertion either targets a row created
 * by this spec or is scoped to "after action X, this row's state
 * matches Y".
 *
 * Lifespan: deleted in Phase 3.7 alongside the rest of Playwright when
 * chromiumoxide takes over.
 */

import { test, expect, type Page } from "@playwright/test";

/** Wait for the WS to connect and the picker UI to mount. */
async function gotoPicker(page: Page) {
  await page.goto("/leptos/");
  // Picker section is present in the initial DOM, but the WS-driven
  // store starts disconnected; wait for the debug snapshot to flip
  // `connected: true` so subsequent sends actually traverse the wire.
  // The debug panel is collapsed by default — open it just for the wait.
  await page.getByTestId("leptos-debug-panel").locator("summary").click();
  await expect(page.getByTestId("leptos-debug-store"))
    .toContainText('"connected": true', { timeout: 5000 });
}

/**
 * Read the conversation store's session_info.dir from the debug
 * snapshot JSON. Ground-truth equivalent of "which session is active".
 * Returns null if the store has no active session.
 */
async function readActiveDir(page: Page): Promise<string | null> {
  const text = await page.getByTestId("leptos-debug-store").innerText();
  const json = JSON.parse(text) as {
    sessionInfo: { dir: string } | null;
  };
  return json.sessionInfo?.dir ?? null;
}

/**
 * Click `+ new session` and wait for the *new* session's dir to land
 * in the conversation store (different from `prev`). Returns the new
 * dir. This is the deterministic "create a session" primitive — no
 * racing on `data-active`.
 */
async function newSession(page: Page, prev: string | null): Promise<string> {
  await page.getByTestId("leptos-session-new").click();
  // Wait for session_info.dir to change to a new value. expect.poll
  // re-reads the debug snapshot until it stabilises on the new dir.
  let next: string | null = null;
  await expect.poll(async () => {
    next = await readActiveDir(page);
    return next !== null && next !== prev;
  }, { timeout: 5000 }).toBeTruthy();
  // And wait for the picker list to render that dir as a row — this
  // is the user-visible end of the create flow.
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${next}"]`),
  ).toBeVisible({ timeout: 3000 });
  return next as unknown as string;
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

test("leptos-picker: clicking + new session creates and activates a row", async ({ page }) => {
  await gotoPicker(page);

  const list = page.getByTestId("leptos-session-list");
  const before = await list.getByTestId("leptos-session-item").count();

  const startDir = await readActiveDir(page);
  const dir = await newSession(page, startDir);

  // List grew, and the new dir is the active row (matches session_info.dir).
  await expect.poll(
    async () => list.getByTestId("leptos-session-item").count(),
    { timeout: 5000 },
  ).toBeGreaterThan(before);

  // Exactly one active row, and it's the dir we just created.
  await expect(page.locator('[data-testid="leptos-session-item"][data-active="true"]'))
    .toHaveCount(1);
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${dir}"]`),
  ).toHaveAttribute("data-active", "true");
});

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

test("leptos-picker: rename updates the row label after server confirms", async ({ page }) => {
  await gotoPicker(page);

  const startDir = await readActiveDir(page);
  const dir = await newSession(page, startDir);

  // Locate the row by its data-session-dir attribute (stable identity,
  // unlike `data-active` which can race).
  const row = page.locator(
    `[data-testid="leptos-session-item"][data-session-dir="${dir}"]`,
  );

  await row.getByTestId("leptos-session-rename").click();
  const input = row.getByTestId("leptos-session-rename-input");
  await expect(input).toBeVisible();

  await input.fill("phase-3-2-renamed");
  await row.getByTestId("leptos-session-rename-submit").click();

  await expect(row.getByTestId("leptos-session-label"))
    .toHaveText("phase-3-2-renamed", { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

test("leptos-picker: delete removes the row after server confirms", async ({ page }) => {
  await gotoPicker(page);

  const startDir = await readActiveDir(page);
  const dir = await newSession(page, startDir);

  const row = page.locator(
    `[data-testid="leptos-session-item"][data-session-dir="${dir}"]`,
  );

  // Auto-accept the window.confirm() the row's delete handler raises.
  page.once("dialog", (d) => d.accept());

  await row.getByTestId("leptos-session-delete").click();

  // The matching row vanishes. Use the data-session-dir attribute as
  // the unique identifier.
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${dir}"]`),
  ).toHaveCount(0, { timeout: 3000 });
});

// ---------------------------------------------------------------------------
// List + active distinction
// ---------------------------------------------------------------------------

test("leptos-picker: only one row is marked active and matches session_info.dir", async ({ page }) => {
  await gotoPicker(page);

  const start = await readActiveDir(page);
  const a = await newSession(page, start);
  const b = await newSession(page, a);

  // Exactly one active row, and it's `b` (the most recent).
  await expect(page.locator('[data-testid="leptos-session-item"][data-active="true"]'))
    .toHaveCount(1);
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${b}"]`),
  ).toHaveAttribute("data-active", "true");

  // `a` exists but is no longer active.
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${a}"]`),
  ).toHaveAttribute("data-active", "false");
});
