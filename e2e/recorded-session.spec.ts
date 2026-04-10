/**
 * Recorded-session replay test.
 *
 * Loads a realistic events.jsonl fixture (trimmed & anonymized from a real
 * session) into the test server's session directory, then reloads the page
 * and asserts the UI renders every block without errors.
 *
 * This catches bugs where persisted event fields differ from the hand-crafted
 * events used in other e2e tests — e.g. the llm_call.request field being
 * absent on disk but always present in fabricated test events.
 */

import { readFileSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";
import { test, expect } from "./fixtures/index.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = join(__dirname, "fixtures", "recorded-session.jsonl");

function loadFixtureLines(): string[] {
  const text = readFileSync(FIXTURE_PATH, "utf-8");
  return text.split("\n").filter((l) => l.trim() !== "");
}

// ---------------------------------------------------------------------------

test("recorded session replays all blocks after page reload", async ({
  page,
  server,
}) => {
  // 1. Navigate to the app first (establishes WebSocket)
  await page.goto("/");
  await expect(page.getByTestId("status-label")).toHaveText("Ready");

  // 2. Write the fixture directly to the session's events.jsonl
  const lines = loadFixtureLines();
  await server.loadFixture(lines);

  // 3. Reload — the server reads from disk and sends history
  await page.reload();

  // Wait for the feed to populate (connected + history replayed)
  const feed = page.getByTestId("feed");

  // Wait for the feed to be populated after history replay
  await page.locator('[data-testid="omega-btn"][data-status="connected"]').waitFor({ timeout: 5000 });

  // 5. Assert we see the expected blocks from both turns

  // Two user_message blocks (one per turn) — with their text visible
  const userBlocks = feed.getByTestId("block-user");
  await expect(userBlocks).toHaveCount(2);
  await expect(userBlocks.first()).toContainText("ping");
  await expect(userBlocks.nth(1)).toContainText("list the files");

  // Turn 1: assistant "pong" — text is now in the llm_response block
  await expect(feed.getByTestId("block-llm-response").first()).toContainText("pong");

  // Turn 2: tool_call block with tool name, then assistant text with file listing
  await expect(feed.getByTestId("block-tool").first()).toContainText("list_files");
  // The fixture has 3 llm_response events total (1 in turn 1, 2 in turn 2)
  await expect(feed.getByTestId("block-llm-response").nth(2)).toContainText("README.md");

  // Both turns should have footer blocks (turn_end)
  const footers = feed.getByTestId("block-turn-end");
  await expect(footers).toHaveCount(2);

  // 6. No render errors — the ErrorBoundary should NOT be visible
  await expect(page.getByTestId("render-error")).toHaveCount(0);
});
