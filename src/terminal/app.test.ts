/**
 * Structural test: every OmegaEvent and StreamSignal variant must have an
 * explicit case in the terminal app's event switch.
 *
 * This is a static/textual check — it reads the source files directly rather
 * than executing any app code. No production code changes required, no stdout
 * stubbing, no refactoring.
 *
 * When a new event type is added to OmegaEvent (or StreamSignal) but the
 * terminal app's switch is not updated, this test fails immediately — before
 * the missing case can reach users as a runtime crash via exhaustiveCheck.
 */

import { describe, it, expect } from "bun:test";
import { readFileSync } from "fs";
import { join } from "path";

const ROOT = join(import.meta.dir, "..", "..");

const eventsSource = readFileSync(join(ROOT, "src/events.ts"), "utf-8");
const appSource    = readFileSync(join(ROOT, "src/terminal/app.ts"), "utf-8");

/**
 * Extract all `type: "..."` literal values from a TypeScript interface block.
 * Matches lines like:  type: "session_start";
 */
function extractEventTypes(source: string): string[] {
  const matches = source.matchAll(/^\s+type:\s+"([^"]+)";/gm);
  return [...matches].map(m => m[1]);
}

const allEventTypes = extractEventTypes(eventsSource);

describe("terminal app — event switch coverage", () => {
  it("finds at least one event type in events.ts (self-check)", () => {
    expect(allEventTypes.length).toBeGreaterThan(0);
  });

  for (const eventType of allEventTypes) {
    it(`has a case for "${eventType}"`, () => {
      expect(appSource).toContain(`case "${eventType}":`);
    });
  }
});
