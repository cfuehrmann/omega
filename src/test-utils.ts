/**
 * Test utilities — convenience factories for unit tests.
 *
 * ## Design rationale
 *
 * Tests use real session files (contextFile + eventsFile) written to a
 * per-call temp directory under the OS temp root. This means:
 *
 *  - File-write paths are exercised on every agent test, not silently skipped.
 *  - If session files ever become load-bearing for UI features (history replay,
 *    detailed event display, session resume), tests will catch regressions.
 *  - No `OMEGA_TEST` coercion or null-path machinery is needed for isolation —
 *    temp dirs under `os.tmpdir()` are never production paths.
 *
 * ## Usage
 *
 *   const { agent, dispose } = makeTestAgent(provider);
 *   afterAll(dispose);
 *
 * The `dispose()` method removes the temp directory. Call it in `afterAll`
 * (or `afterEach` if each test makes its own agent). For suites where a single
 * agent is shared across all tests, one `afterAll` is sufficient.
 *
 * ## Secondary safety net
 *
 * `OMEGA_TEST=1` (set by the Bun preload in `bunfig.toml`) and
 * `assertNotProductionPath()` in `test-guard.ts` remain as belt-and-suspenders:
 * they will throw loudly if any test accidentally writes to `.omega/sessions/`.
 * They are no longer the *primary* isolation mechanism — the temp dir is.
 */

import { Agent, type StreamProvider } from "./agent.js";
import { callOpenAi } from "./openai.js";
import { mkdtempSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

let _counter = 0;

export interface TestAgent {
  agent: Agent;
  /** Absolute path to the temp session directory. */
  sessionDir: string;
  /** Absolute path to context.jsonl inside sessionDir. */
  contextFile: string;
  /** Absolute path to events.jsonl inside sessionDir. */
  eventsFile: string;
  /** Remove the temp directory. Call in afterAll / afterEach. */
  dispose: () => void;
}

/**
 * Create an Agent backed by real session files in a per-call temp directory.
 *
 * @param streamProvider  Optional mock stream provider. If omitted, any API
 *                        call will throw (which is fine for tests that don't
 *                        exercise the streaming path).
 * @param openAiCaller    Optional mock OpenAI caller.
 */
export function makeTestAgent(
  streamProvider?: StreamProvider,
  openAiCaller?: typeof callOpenAi
): TestAgent {
  const sessionDir = mkdtempSync(
    join(tmpdir(), `omega-test-${Date.now()}-${++_counter}-`)
  );
  const contextFile = join(sessionDir, "context.jsonl");
  const eventsFile = join(sessionDir, "events.jsonl");

  const agent = new Agent(
    streamProvider,
    null,          // _sessionDir (unused legacy positional)
    openAiCaller,
    contextFile,
    eventsFile,
  );

  const dispose = () => {
    try {
      rmSync(sessionDir, { recursive: true, force: true });
    } catch {
      // best-effort
    }
  };

  return { agent, sessionDir, contextFile, eventsFile, dispose };
}
