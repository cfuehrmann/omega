/**
 * Control-API helpers for the mock-omega-server real-server fixture.
 *
 * The Rust binary at `rust/crates/omega-mock-server/` hosts an internal
 * Anthropic-shaped SSE fake on a random 127.0.0.1 port and exposes a
 * tiny control HTTP API on port 3004:
 *
 *   • `POST /control/script`      — replace the queue of mock responses.
 *   • `POST /control/reset-calls` — clear captured-call history.
 *
 * (`GET /control/llm-calls` is exposed by the binary and used by the
 * Phase-4 chromiumoxide port, but no surviving Playwright spec needs
 * it post-3.7.)
 *
 * Each `POST /v1/messages` from the production AnthropicProvider pops
 * one entry from the script queue and replies with an Anthropic-shaped
 * SSE stream. Tests load the script they expect *before* triggering the
 * UI action that causes the agent to call the LLM.
 *
 * The `Step` type and its serialisation match the `MockResponse` enum
 * in `rust/crates/omega-mock-server/src/fake.rs` (internally tagged on
 * `kind`, camelCase fields).
 */

const CONTROL_URL = "http://localhost:3004";

type Step =
  | { kind: "text"; text: string }
  | { kind: "slowText"; text: string; chunks: number; delayMs: number }
  | { kind: "toolUse"; id: string; name: string; input: unknown }
  | {
      kind: "toolUseMulti";
      tools: Array<{ id: string; name: string; input: unknown }>;
    }
  | { kind: "httpError"; status: number; body: string };

/**
 * Replace the mock response queue with `steps`. Returns once the server
 * has applied the change. Throws on non-2xx.
 */
export async function loadScript(steps: Step[]): Promise<void> {
  const res = await fetch(`${CONTROL_URL}/control/script`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(steps),
  });
  if (!res.ok) {
    throw new Error(`load-script failed: ${res.status} ${await res.text()}`);
  }
}

/** Clear the captured-call history. */
export async function resetCalls(): Promise<void> {
  await fetch(`${CONTROL_URL}/control/reset-calls`, { method: "POST" });
}

// ---------------------------------------------------------------------------
// Named scripts — one per historical scenario from the old MockProvider.
// Tests call e.g. `await loadScript(SCRIPTS.multiTool())` instead of relying
// on the binary to recognise a magic user-message string.
// ---------------------------------------------------------------------------

const sleep = (s: string) => ({ command: `sleep ${s}` });

export const SCRIPTS = {
  /** Single text turn. The default "ping → pong" smoke. */
  pong: (): Step[] => [{ kind: "text", text: "pong" }],

  /** One run_command(sleep 10) — abort regression. */
  abortSleep: (): Step[] => [
    { kind: "toolUse", id: "toolu_sleep_abort", name: "run_command", input: sleep("10") },
  ],

  /** Three sequential tool turns then a final text. */
  multiTool: (): Step[] => [
    { kind: "toolUse", id: "toolu_mt_1", name: "run_command", input: sleep("0.6") },
    { kind: "toolUse", id: "toolu_mt_2", name: "run_command", input: sleep("0.6") },
    { kind: "toolUse", id: "toolu_mt_3", name: "run_command", input: sleep("0.6") },
    { kind: "text", text: "done multi" },
  ],

  /** Two concurrent tools (one fast, one slow) then a final text. */
  concurrentTools: (): Step[] => [
    {
      kind: "toolUseMulti",
      tools: [
        { id: "toolu_ct_fast", name: "run_command", input: sleep("0.1") },
        { id: "toolu_ct_slow", name: "run_command", input: sleep("1.5") },
      ],
    },
    { kind: "text", text: "done concurrent" },
  ],

  /** Long chunked text response, used to exercise pause-during-streaming. */
  longStream: (): Step[] => [
    {
      kind: "slowText",
      text:
        "This is a deliberately long streaming response emitted in chunks. done stream",
      chunks: 8,
      delayMs: 100,
    },
  ],

  /** Four tool turns then a final text (room for two pause cycles). */
  twoPauses: (): Step[] => [
    { kind: "toolUse", id: "toolu_tp_1", name: "run_command", input: sleep("0.6") },
    { kind: "toolUse", id: "toolu_tp_2", name: "run_command", input: sleep("0.6") },
    { kind: "toolUse", id: "toolu_tp_3", name: "run_command", input: sleep("0.6") },
    { kind: "toolUse", id: "toolu_tp_4", name: "run_command", input: sleep("0.6") },
    { kind: "text", text: "done two pauses" },
  ],

  /**
   * One tool turn, one final text, then a synthetic resumption summary
   * — covers the full RESUME_BASIS_TEST flow including the post-resume
   * `/v1/messages` call the server fires with a `Summarise the coding
   * session` system prompt.
   */
  resumeBasis: (): Step[] => [
    { kind: "toolUse", id: "toolu_rb_1", name: "run_command", input: sleep("0.3") },
    { kind: "text", text: "done basis" },
    {
      kind: "text",
      text:
        "<summary>Resumed session summary.</summary>\n<description>Resumed work.</description>",
    },
  ],
};
