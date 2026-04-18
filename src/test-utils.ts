/**
 * Test utilities — convenience factories for unit tests.
 *
 * ## Design rationale
 *
 * Tests use real session files written to `.omega/test-sessions/` — the same
 * `makeSessionDir()` call that production code uses, but with `TEST_SESSIONS_ROOT`
 * instead of `SESSIONS_ROOT`. This means:
 *
 *  - Write paths are exercised on every agent test, not silently skipped.
 *  - Read/replay regressions are caught: test sessions accumulate as real,
 *    inspectable artifacts just like production sessions do.
 *  - If session files ever become load-bearing (history replay, session resume),
 *    tests will catch regressions because the files are real and persistent.
 *
 * Isolation from production is achieved by path (`.omega/test-sessions/` vs
 * `.omega/sessions/`), not by deleting output or routing to `/tmp`. Each test
 * run gets a uniquely named subdirectory (timestamp + counter via
 * `makeSessionDir()`), so tests can run fully in parallel without conflicts.
 *
 * The `dispose()` method is a no-op — test sessions are preserved so they
 * can be inspected after a failure. Periodic pruning of old test sessions is
 * a separate concern (cron job, `just prune`, etc.).
 *
 * ## Usage
 *
 *   const { agent, dispose } = await makeTestAgent(provider);
 *   afterAll(dispose);
 *
 * Note: `makeTestAgent` is async because `makeSessionDir` is async.
 *
 * ## Secondary safety net
 *
 * `OMEGA_TEST=1` (set by the Bun preload in `bunfig.toml`) and
 * `assertNotProductionPath()` in `test-guard.ts` remain as belt-and-suspenders:
 * they will throw loudly if any test accidentally writes to `.omega/sessions/`.
 * They are not the primary isolation mechanism — path separation is.
 */

import { Agent, type CreateMessageStream } from "./agent.js";
import { makeSessionDir, TEST_SESSIONS_ROOT } from "./session-dir.js";

export interface TestAgent {
  agent: Agent;
  /** Path to the test session directory under .omega/test-sessions/. */
  sessionDir: string;
  /** Path to context.jsonl inside sessionDir. */
  contextFile: string;
  /** Path to events.jsonl inside sessionDir. */
  eventsFile: string;
  /**
   * No-op. Test session dirs are preserved for inspection.
   * Call in afterAll / afterEach for forward-compatibility.
   */
  dispose: () => void;
}

let _counter = 0;

/**
 * Create an Agent backed by real session files in `.omega/test-sessions/`.
 *
 * Each call creates a fresh uniquely-named session directory (via
 * `makeSessionDir()`), so concurrent tests never collide.
 *
 * @param createMessageStream  Optional mock stream function. If omitted, any LLM
 *                        call will throw (fine for tests that don't exercise
 *                        the streaming path).
 */
export async function makeTestAgent(
  createMessageStream?: CreateMessageStream
): Promise<TestAgent> {
  // Incorporate a monotonic counter into the timestamp to guarantee uniqueness
  // even when multiple calls happen within the same millisecond.
  const now = new Date(Date.now() + ++_counter);
  const { dir, contextFile, eventsFile } = await makeSessionDir(
    now,
    TEST_SESSIONS_ROOT,
  );

  const agent = new Agent(createMessageStream, contextFile, eventsFile);

  return {
    agent,
    sessionDir: dir,
    contextFile,
    eventsFile,
    dispose: () => {
      /* no-op: test sessions are preserved for inspection */
    },
  };
}
