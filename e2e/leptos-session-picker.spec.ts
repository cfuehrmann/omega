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

/**
 * Wait for the WS to connect, then ensure the picker panel is open.
 *
 * Phase 3.10 TODO-F flipped `PickerOpen`'s default to `false` so a
 * browser refresh of an active session lands directly in the
 * conversation feed. The picker auto-opens only when `connected &&
 * session_info.is_none()` (genuinely fresh server). Most tests run
 * against `mock-omega-server` which has an active session at first
 * connect, so the picker stays closed and the spec must open it
 * explicitly. `gotoPicker` does that for any spec that needs to
 * interact with picker rows immediately.
 */
async function gotoPicker(page: Page) {
  await page.goto("/leptos/");
  // Wait for the WS to connect: `data-connected` on <main> flips to
  // "true" once `SessionStore::apply(WsMessage::Ready)` fires.
  await expect(page.locator('main[data-connected="true"]'))
    .toBeAttached({ timeout: 5000 });
  // Open the picker if it's not already open (TODO-F may have
  // auto-opened it on a fresh server with no active session).
  const picker = page.getByTestId("leptos-session-picker");
  if ((await picker.count()) === 0) {
    await page.getByTestId("leptos-composer-sessions").click();
  }
  await expect(picker).toBeVisible({ timeout: 2000 });
}

/**
 * Read the active session dir from `data-active-session-dir` on <main>.
 * This attribute is always in the DOM (not inside the picker panel)
 * so it works whether the picker is open or closed. Updated reactively
 * from `SessionStore.session_info.dir` (Phase 3.9 TODO-4).
 */
async function readActiveDir(page: Page): Promise<string | null> {
  const val = await page.locator("main").getAttribute("data-active-session-dir");
  return val || null;
}

/**
 * Click `+ new session` and wait for the *new* session's dir to land
 * in `data-active-session-dir` on <main> (different from `prev`).
 * Returns the new dir.
 *
 * NOTE (Phase 3.9 TODO-2): clicking `+ new session` auto-closes the
 * picker, so the picker row for the new session is not visible after
 * this call. Open the picker again (via `leptos-composer-sessions`)
 * before asserting on picker rows.
 *
 * NOTE (Phase 3.10 TODO-F): picker default is closed; this helper
 * opens it first if necessary so it can also be used post-`gotoPicker`
 * after a chain of operations that closed it.
 */
async function newSession(page: Page, prev: string | null): Promise<string> {
  if ((await page.getByTestId("leptos-session-picker").count()) === 0) {
    await page.getByTestId("leptos-composer-sessions").click();
  }
  await page.getByTestId("leptos-session-new").click();
  // Wait for data-active-session-dir to change to a new value.
  let next: string | null = null;
  await expect.poll(async () => {
    next = await readActiveDir(page);
    return next !== null && next !== prev;
  }, { timeout: 5000 }).toBeTruthy();
  return next as unknown as string;
}

/** Open the session picker via the composer "Sessions" button. */
async function openPicker(page: Page) {
  await page.getByTestId("leptos-composer-sessions").click();
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible({ timeout: 2000 });
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

test("leptos-picker: clicking + new session creates and activates a row", async ({ page }) => {
  await gotoPicker(page);

  const startDir = await readActiveDir(page);
  const dir = await newSession(page, startDir);

  // `+ new session` auto-closes the picker (Phase 3.9 TODO-2).
  await expect(page.getByTestId("leptos-session-picker"))
    .toHaveCount(0);

  // Re-open picker to inspect the list.
  await openPicker(page);
  const list = page.getByTestId("leptos-session-list");

  // The new dir is the active row (matches session_info.dir).
  await expect(
    page.locator(`[data-testid="leptos-session-item"][data-session-dir="${dir}"]`),
  ).toHaveAttribute("data-active", "true");

  // Exactly one active row.
  await expect(page.locator('[data-testid="leptos-session-item"][data-active="true"]'))
    .toHaveCount(1);
});

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

test("leptos-picker: rename updates the row label after server confirms", async ({ page }) => {
  await gotoPicker(page);

  const startDir = await readActiveDir(page);
  const dir = await newSession(page, startDir);

  // Re-open the picker (auto-closed by + new session).
  await openPicker(page);

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

  // Re-open the picker (auto-closed by + new session).
  await openPicker(page);

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

  // Re-open picker to create second session.
  await openPicker(page);
  const b = await newSession(page, a);

  // Re-open picker to inspect rows.
  await openPicker(page);

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

// ---------------------------------------------------------------------------
// Phase 3.9 — open/close cycle (TODO-1)
// ---------------------------------------------------------------------------

test("leptos-picker: \u2715 close button dismisses the picker; Sessions button re-opens", async ({ page }) => {
  await gotoPicker(page);

  // gotoPicker guarantees the picker is open.
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible();

  // ✕ close button dismisses the picker.
  await page.getByTestId("leptos-picker-close").click();
  await expect(page.getByTestId("leptos-session-picker")).toHaveCount(0);

  // Backdrop is also gone.
  await expect(page.getByTestId("leptos-picker-backdrop")).toHaveCount(0);

  // Sessions button in the composer re-opens it.
  await page.getByTestId("leptos-composer-sessions").click();
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible({ timeout: 2000 });
});

test("leptos-picker: clicking outside the panel (backdrop) closes the picker", async ({ page }) => {
  await gotoPicker(page);
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible();

  // Click the backdrop div itself (not the panel). Use the backdrop
  // testid and click at its top-left corner to land outside the panel.
  const backdrop = page.getByTestId("leptos-picker-backdrop");
  await backdrop.click({ position: { x: 5, y: 5 } });
  await expect(page.getByTestId("leptos-session-picker")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Phase 3.9 — auto-close on Reset / Resume (TODO-2)
// ---------------------------------------------------------------------------

test("leptos-picker: + new session auto-closes the picker", async ({ page }) => {
  await gotoPicker(page);
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible();

  const prev = await readActiveDir(page);
  // Click + new session while picker is open.
  await page.getByTestId("leptos-session-new").click();

  // Picker must close immediately (before the server ack arrives).
  await expect(page.getByTestId("leptos-session-picker")).toHaveCount(0);

  // The session store eventually reflects the new dir.
  await expect.poll(async () => {
    const d = await readActiveDir(page);
    return d !== null && d !== prev;
  }, { timeout: 5000 }).toBeTruthy();
});

test("leptos-picker: resume auto-closes the picker", async ({ page }) => {
  await gotoPicker(page);

  // Create two sessions so we have a non-active row to resume.
  const start = await readActiveDir(page);
  const a = await newSession(page, start);

  // Re-open picker (closed by newSession) and create a second one
  // so `a` is no longer active.
  await openPicker(page);
  await newSession(page, a);

  // Re-open picker and click Resume on row `a` (now inactive).
  await openPicker(page);
  const rowA = page.locator(
    `[data-testid="leptos-session-item"][data-session-dir="${a}"]`,
  );
  await expect(rowA).toBeVisible();
  await rowA.getByTestId("leptos-session-resume").click();

  // Picker must close on resume.
  await expect(page.getByTestId("leptos-session-picker")).toHaveCount(0, { timeout: 3000 });
});

test("leptos-picker: rename does NOT close the picker", async ({ page }) => {
  await gotoPicker(page);

  const prev = await readActiveDir(page);
  const dir = await newSession(page, prev);
  await openPicker(page);

  const row = page.locator(
    `[data-testid="leptos-session-item"][data-session-dir="${dir}"]`,
  );
  await row.getByTestId("leptos-session-rename").click();

  // Picker must still be open during rename.
  await expect(page.getByTestId("leptos-session-picker")).toBeVisible();
});
