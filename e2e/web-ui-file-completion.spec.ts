/**
 * E2e tests for @-path file completion in the message textarea.
 *
 * Covers:
 *  1. Typing @ opens the dropdown
 *  2. Typing more characters narrows the list
 *  3. ArrowDown / ArrowUp move the highlight (with wrap-around)
 *  4. Enter with a highlighted file accepts it, closes dropdown, does NOT send
 *  5. Enter with nothing highlighted just closes the dropdown, does NOT send
 *  6. Esc closes the dropdown, keeps text
 *  7. "/" on a highlighted directory drills in (dropdown re-queries)
 *  8. Resumability: after Esc, typing "/" reopens dropdown for the next level
 *  9. Clicking an item in the dropdown accepts it
 * 10. Clicking outside the dropdown closes it
 * 11. Tab / Shift+Tab move the highlight (with wrap-around)
 * 12. Tab does not shift browser focus while the dropdown is open
 */

import { test, expect } from "./fixtures/index.js";

const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

async function setup(page: import("@playwright/test").Page) {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
}

const dropdown = (page: import("@playwright/test").Page) =>
  page.locator(".fc-dropdown");

const items = (page: import("@playwright/test").Page) =>
  page.locator(".fc-item");

const highlighted = (page: import("@playwright/test").Page) =>
  page.locator(".fc-item.fc-hl");

// ---------------------------------------------------------------------------
// 1. Typing @ opens the dropdown
// ---------------------------------------------------------------------------

test("typing @ opens the file completion dropdown", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });
  // Known project entries must appear
  await expect(items(page).filter({ hasText: "src/" })).toBeVisible({ timeout: 2000 });
});

// ---------------------------------------------------------------------------
// 2. Typing more characters narrows the list
// ---------------------------------------------------------------------------

test("typing after @ filters the dropdown items", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@src");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });
  // Items shown should all start with "src"
  const count = await items(page).count();
  expect(count).toBeGreaterThan(0);
  for (let i = 0; i < count; i++) {
    await expect(items(page).nth(i)).toContainText("src");
  }
});

test("typing a non-matching filter hides the dropdown", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@zzz_no_match_ever");
  // Dropdown should not be visible (no results)
  await expect(dropdown(page)).not.toBeVisible({ timeout: 2000 });
});

// ---------------------------------------------------------------------------
// 3. ArrowDown / ArrowUp move the highlight
// ---------------------------------------------------------------------------

test("ArrowDown highlights the first item, subsequent presses move down", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Initially nothing is highlighted
  await expect(highlighted(page)).not.toBeVisible();

  // First ArrowDown → first item highlighted
  await page.keyboard.press("ArrowDown");
  await expect(highlighted(page)).toBeVisible({ timeout: 1000 });
  const firstText = await highlighted(page).textContent();

  // Second ArrowDown → second item highlighted (different from first)
  await page.keyboard.press("ArrowDown");
  await expect(highlighted(page)).toBeVisible();
  const secondText = await highlighted(page).textContent();
  expect(secondText).not.toBe(firstText);
});

test("ArrowUp wraps from first item to last", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // ArrowDown to first item, then ArrowUp should wrap to last
  await page.keyboard.press("ArrowDown");
  const firstText = await highlighted(page).textContent();

  await page.keyboard.press("ArrowUp");
  const wrappedText = await highlighted(page).textContent();
  // Wrapped to last — should differ from first (assuming >1 items, which is certain for "@")
  expect(wrappedText).not.toBe(firstText);
});

// ---------------------------------------------------------------------------
// 11. Tab / Shift+Tab move the highlight (with wrap-around)
// ---------------------------------------------------------------------------

test("Tab highlights the first item, subsequent presses move down", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Initially nothing is highlighted
  await expect(highlighted(page)).not.toBeVisible();

  // First Tab → first item highlighted
  await page.keyboard.press("Tab");
  await expect(highlighted(page)).toBeVisible({ timeout: 1000 });
  const firstText = await highlighted(page).textContent();

  // Second Tab → second item highlighted (different from first)
  await page.keyboard.press("Tab");
  await expect(highlighted(page)).toBeVisible();
  const secondText = await highlighted(page).textContent();
  expect(secondText).not.toBe(firstText);
});

test("Shift+Tab wraps from first item to last", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Tab to first item, then Shift+Tab should wrap to last
  await page.keyboard.press("Tab");
  const firstText = await highlighted(page).textContent();

  await page.keyboard.press("Shift+Tab");
  const wrappedText = await highlighted(page).textContent();
  // Wrapped to last — should differ from first (assuming >1 items, which is certain for "@")
  expect(wrappedText).not.toBe(firstText);
});

// ---------------------------------------------------------------------------
// 12. Tab does not shift browser focus while the dropdown is open
// ---------------------------------------------------------------------------

test("Tab does not move browser focus away from the textarea while dropdown is open", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  await page.keyboard.press("Tab");

  // Textarea must still be the active element
  await expect(textarea).toBeFocused();
  // Dropdown must still be visible
  await expect(dropdown(page)).toBeVisible();
});

// ---------------------------------------------------------------------------
// 4. Enter with highlighted file: accepts, closes dropdown, does NOT send
// ---------------------------------------------------------------------------

test("Enter on a highlighted file accepts it and does not send the message", async ({ page, server }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  // Type prefix that matches a known file: "backlog.md" starts with "backlog"
  await textarea.pressSequentially("@backlog");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Highlight the first (and only) match
  await page.keyboard.press("ArrowDown");
  await expect(highlighted(page)).toBeVisible();
  await expect(highlighted(page)).toContainText("backlog.md");

  // Enter accepts the item
  await page.keyboard.press("Enter");

  // Dropdown should close
  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });

  // Textarea should contain the accepted path
  await expect(textarea).toHaveValue("@backlog.md");

  // No message should have been sent to the server
  const msgs = await server.drainMessages();
  expect(msgs).toHaveLength(0);
});

// ---------------------------------------------------------------------------
// 5. Enter with nothing highlighted: closes dropdown, does NOT send
// ---------------------------------------------------------------------------

test("Enter with no highlight just closes the dropdown without sending", async ({ page, server }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@src");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Confirm nothing highlighted
  await expect(highlighted(page)).not.toBeVisible();

  await page.keyboard.press("Enter");

  // Dropdown closes
  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });

  // No message sent
  const msgs = await server.drainMessages();
  expect(msgs).toHaveLength(0);
});

// ---------------------------------------------------------------------------
// 6. Esc closes the dropdown and keeps text
// ---------------------------------------------------------------------------

test("Esc closes the dropdown and keeps the typed text", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@src");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  await page.keyboard.press("Escape");

  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });
  // Text is preserved
  await expect(textarea).toHaveValue("@src");
});

// ---------------------------------------------------------------------------
// 7. "/" on a highlighted directory drills in
// ---------------------------------------------------------------------------

test("pressing / on a highlighted directory drills into it", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  // "src" should match "src/" directory
  await textarea.pressSequentially("@src");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Highlight the "src/" entry
  await page.keyboard.press("ArrowDown");
  await expect(highlighted(page)).toContainText("src/");

  // Press "/" — should accept "src/" and list its contents
  await page.keyboard.press("/");

  // Dropdown should still be open (drilling into a directory)
  await expect(dropdown(page)).toBeVisible({ timeout: 2000 });

  // Textarea should now show "@src/"
  await expect(textarea).toHaveValue("@src/");

  // Items should now be contents of src/ (e.g. "src/agent.ts" or "src/web/")
  const count = await items(page).count();
  expect(count).toBeGreaterThan(0);
  const firstItem = await items(page).first().textContent();
  expect(firstItem).toMatch(/^src\//);
});

// ---------------------------------------------------------------------------
// 8. Resumability: Esc then type "/" reopens dropdown
// ---------------------------------------------------------------------------

test("typing / after Esc on a @-path token reopens the dropdown", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@src");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Close with Esc
  await page.keyboard.press("Escape");
  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });

  // Cursor is right after "src" — type "/" to extend the path
  await textarea.pressSequentially("/");

  // Dropdown should reopen showing contents of src/
  await expect(dropdown(page)).toBeVisible({ timeout: 2000 });
  await expect(textarea).toHaveValue("@src/");
  const firstItem = await items(page).first().textContent();
  expect(firstItem).toMatch(/^src\//);
});

// ---------------------------------------------------------------------------
// 9. Mouse click on an item accepts it
// ---------------------------------------------------------------------------

test("clicking a dropdown item accepts it without closing via keyboard", async ({ page, server }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@backlog");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Click the item
  await items(page).filter({ hasText: "backlog.md" }).click();

  // Dropdown closes, path inserted
  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });
  await expect(textarea).toHaveValue("@backlog.md");

  // No message sent
  const msgs = await server.drainMessages();
  expect(msgs).toHaveLength(0);
});

// ---------------------------------------------------------------------------
// 10. Clicking outside closes the dropdown
// ---------------------------------------------------------------------------

test("clicking outside the textarea closes the dropdown", async ({ page }) => {
  await setup(page);
  const textarea = page.locator("textarea");
  await textarea.click();
  await textarea.pressSequentially("@");
  await expect(dropdown(page)).toBeVisible({ timeout: 3000 });

  // Click somewhere outside (the page header / feed area)
  await page.locator(".feed").click();

  await expect(dropdown(page)).not.toBeVisible({ timeout: 1000 });
});
