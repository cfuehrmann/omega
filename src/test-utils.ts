/**
 * Test utilities — convenience factories for unit tests.
 *
 * Import `makeTestAgent` instead of constructing `Agent` directly in tests.
 * This ensures all file paths are null (no production writes) and makes the
 * right thing the easy thing for future test authors.
 *
 * Safety is structural: Agent's constructor already coerces undefined paths
 * to null when OMEGA_TEST=1 (layer c). makeTestAgent is an ergonomic layer
 * on top — it also makes intent explicit and removes boilerplate.
 */

import { Agent, type StreamProvider } from "./agent.js";
import { callOpenAi } from "./openai.js";

/**
 * Create an Agent suitable for unit tests.
 *
 * All production file paths (contextFile, eventsFile) are set to
 * null — no files will be written regardless of OMEGA_TEST.
 *
 * @param streamProvider  Optional mock stream provider. If omitted, any API
 *                        call will throw (which is fine for tests that don't
 *                        exercise the streaming path).
 * @param openAiCaller    Optional mock OpenAI caller. Defaults to a no-op
 *                        stub that throws, matching the Anthropic default.
 */
export function makeTestAgent(
  streamProvider?: StreamProvider,
  openAiCaller?: typeof callOpenAi
): Agent {
  return new Agent(
    streamProvider,
    null,          // _sessionDir (unused legacy positional)
    openAiCaller,  // undefined → real callOpenAi, but paths are null so no writes
    null,          // contextFile
    null,          // eventsFile
  );
}
