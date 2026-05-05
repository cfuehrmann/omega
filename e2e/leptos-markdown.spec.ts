/**
 * Phase 3.6 — Leptos markdown / mermaid e2e parity tests.
 *
 * Drives the conversation feed at `/leptos/` against the real-server
 * project (port 3003). Validates that assistant text rendered into
 * the feed gains:
 *
 * 1. Markdown affordances (paragraphs, code blocks, lists, tables,
 *    links, GFM strikethrough) — same surface as the SolidJS
 *    `MdBody` component.
 * 2. Diff colouring on `language-diff` / `language-patch` fenced
 *    blocks — same `.diff-add` / `.diff-del` / `.diff-hunk` /
 *    `.diff-file` / `.diff-ctx` class names as the SolidJS UI.
 * 3. Mermaid lazy-load on first detection — the `mermaid-pending`
 *    `<pre>` is replaced by a `mermaid-wrapper` carrying the
 *    rendered SVG (or an error notice + raw source on failure).
 * 4. Inline raw HTML in markdown source is escaped (`<script>` →
 *    visible text, never live DOM).
 *
 * Mirrors `e2e/web-ui-mermaid.spec.ts` and the assistant-text
 * fixtures from `e2e/web-ui-4.spec.ts` so the two bundles can be
 * compared side-by-side at parity time.
 *
 * Lifespan: deleted in Phase 3.7 alongside the rest of Playwright
 * when chromiumoxide takes over.
 */

import { test, expect, type Page } from "@playwright/test";
import { loadScript, resetCalls } from "./fixtures/real-server-control";

async function gotoFeed(page: Page) {
  await page.goto("/leptos/");
  await expect(page.locator('main[data-connected="true"]'))
    .toBeAttached({ timeout: 5000 });
}

async function readActiveDir(page: Page): Promise<string | null> {
  const val = await page.locator("main").getAttribute("data-active-session-dir");
  return val || null;
}

async function newSession(page: Page, prev: string | null): Promise<string> {
  await page.getByTestId("leptos-session-new").click();
  let next: string | null = null;
  await expect.poll(async () => {
    next = await readActiveDir(page);
    return next !== null && next !== prev;
  }, { timeout: 5000 }).toBeTruthy();
  return next as unknown as string;
}

async function sendMessage(page: Page, content: string) {
  const input = page.getByTestId("leptos-composer-input");
  await input.fill(content);
  await input.press("Enter");
}

/**
 * Drive a single assistant-only turn that emits exactly the given
 * markdown text in the `llm_response` event.
 */
async function runAssistantTurn(page: Page, markdown: string) {
  await resetCalls();
  await loadScript([{ kind: "text", text: markdown }]);
  await gotoFeed(page);
  const startDir = await readActiveDir(page);
  await newSession(page, startDir);
  await sendMessage(page, "render markdown");
  // Wait for the turn to settle.
  await expect(
    page.getByTestId("leptos-feed").locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 15000 });
}

const MD_BODY = '[data-testid="md-body"]';

// ---------------------------------------------------------------------------
// Markdown surfaces
// ---------------------------------------------------------------------------

test("leptos-md: assistant text renders inside an md-body", async ({ page }) => {
  await runAssistantTurn(page, "**bold** and `inline code`");
  const body = page.locator(MD_BODY);
  await expect(body).toBeVisible();
  await expect(body.locator("strong")).toHaveText("bold");
  await expect(body.locator("code")).toHaveText("inline code");
});

test("leptos-md: paragraph + lists + headings", async ({ page }) => {
  await runAssistantTurn(
    page,
    "## Steps\n\nDo the following:\n\n- one\n- two\n- three\n"
  );
  const body = page.locator(MD_BODY);
  await expect(body.locator("h2")).toHaveText("Steps");
  await expect(body.locator("ul li")).toHaveCount(3);
  await expect(body.locator("ul li").first()).toHaveText("one");
});

test("leptos-md: GFM table renders", async ({ page }) => {
  await runAssistantTurn(
    page,
    "| col a | col b |\n|-------|-------|\n| 1     | 2     |\n"
  );
  const body = page.locator(MD_BODY);
  await expect(body.locator("table")).toBeVisible();
  await expect(body.locator("th").first()).toHaveText("col a");
  await expect(body.locator("td").first()).toHaveText("1");
});

test("leptos-md: links keep their href", async ({ page }) => {
  await runAssistantTurn(
    page,
    "see [omega](https://example.com/foo)"
  );
  const link = page.locator(MD_BODY).locator("a");
  await expect(link).toHaveText("omega");
  await expect(link).toHaveAttribute("href", "https://example.com/foo");
});

test("leptos-md: fenced code block keeps the language class", async ({ page }) => {
  await runAssistantTurn(
    page,
    "```rust\nlet x = 1;\n```\n"
  );
  const code = page.locator(MD_BODY).locator("pre code.language-rust");
  await expect(code).toBeAttached({ timeout: 3000 });
  await expect(code).toContainText("let x = 1;");
});

test("leptos-md: raw HTML in markdown source is escaped", async ({ page }) => {
  await runAssistantTurn(
    page,
    "hello <script>alert(1)</script>"
  );
  const body = page.locator(MD_BODY);
  // The script tag must NOT execute as DOM — visible as text instead.
  await expect(body.locator("script")).toHaveCount(0);
  await expect(body).toContainText("<script>alert(1)</script>");
});

// ---------------------------------------------------------------------------
// Diff colouring
// ---------------------------------------------------------------------------

test("leptos-md: diff block gets line-level classes", async ({ page }) => {
  await runAssistantTurn(
    page,
    "```diff\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n- old\n+ new\n  ctx\n```\n"
  );
  const body = page.locator(MD_BODY);
  await expect(body.locator(".diff-file")).toHaveCount(2, { timeout: 5000 });
  await expect(body.locator(".diff-hunk")).toHaveCount(1);
  await expect(body.locator(".diff-add")).toHaveCount(1);
  await expect(body.locator(".diff-del")).toHaveCount(1);
  await expect(body.locator(".diff-ctx")).toHaveCount(1);
  // The wrapper pre gains the diff-block marker.
  await expect(body.locator("pre.diff-block")).toBeAttached();
  await expect(body.locator('pre[data-testid="diff-block"]')).toBeAttached();
});

test("leptos-md: patch language tag also triggers diff colouring", async ({ page }) => {
  await runAssistantTurn(page, "```patch\n+ added\n```\n");
  await expect(page.locator(MD_BODY).locator(".diff-add")).toBeAttached({
    timeout: 5000,
  });
});

// ---------------------------------------------------------------------------
// Mermaid lazy-load
// ---------------------------------------------------------------------------

test("leptos-md: mermaid block renders an SVG diagram", async ({ page }) => {
  await runAssistantTurn(
    page,
    "```mermaid\ngraph LR\n  A --> B\n```\n"
  );
  const wrapper = page.getByTestId("mermaid-wrapper");
  // Lazy-loaded — give it a generous window. The mermaid library
  // pulls from a CDN at first use; localhost CDN probes can be
  // slow in CI so 15 s is the same budget the SolidJS spec uses.
  await wrapper.waitFor({ timeout: 15000 });
  const svg = page.getByTestId("mermaid-diagram").locator("svg");
  await expect(svg).toBeAttached({ timeout: 5000 });
});

test("leptos-md: invalid mermaid surfaces an error notice + raw source", async ({ page }) => {
  await runAssistantTurn(
    page,
    "```mermaid\nthis is not valid mermaid syntax !!!\n```\n"
  );
  const wrapper = page.getByTestId("mermaid-wrapper");
  await wrapper.waitFor({ timeout: 15000 });
  await expect(page.getByTestId("mermaid-error-notice")).toBeAttached({
    timeout: 5000,
  });
  await expect(page.getByTestId("mermaid-error-notice")).toContainText(
    "⚠ Mermaid error"
  );
  await expect(page.getByTestId("mermaid-source")).toContainText(
    "this is not valid mermaid syntax"
  );
});

// ---------------------------------------------------------------------------
// Streaming text stays plain (no markdown render)
// ---------------------------------------------------------------------------

test("leptos-md: streaming overlay renders raw text, not markdown", async ({ page }) => {
  // The streaming overlay (`leptos-streaming-text`) shows the text
  // as it arrives. Markdown rendering applies only to the settled
  // `llm_response` block — mirrors SolidJS where MdBody only mounts
  // after turn_end.
  await resetCalls();
  await loadScript([
    {
      kind: "slowText",
      text: "**still streaming** and growing",
      chunks: 4,
      delayMs: 80,
    },
  ]);
  await gotoFeed(page);
  const startDir = await readActiveDir(page);
  await newSession(page, startDir);
  await sendMessage(page, "render streaming");

  const overlay = page.getByTestId("leptos-streaming-text");
  await expect(overlay).toBeVisible({ timeout: 5000 });
  // The overlay holds the raw stars; no <strong> mounting yet.
  await expect(overlay.locator("strong")).toHaveCount(0);
  await expect(overlay).toContainText("**still streaming**");

  // After the turn settles, the persisted block renders markdown.
  await expect(
    page.getByTestId("leptos-feed").locator('[data-event-type="turn_end"]')
  ).toHaveCount(1, { timeout: 10000 });
  await expect(
    page.locator(MD_BODY).locator("strong")
  ).toHaveText("still streaming");
});
