/**
 * Session resumption — basis extraction and LLM summarisation.
 *
 * The flow for continuing a previous session:
 *
 *   1. Read the previous session's events.jsonl → OmegaEvent[]
 *   2. `extractResumptionBasis(events)` → a structured markdown text
 *      (the "basis") containing only the information relevant for summary.
 *   3. `Agent.performResumption()` streams the LLM summarisation, yielding
 *      live text chunks plus the final `session_resumed` event.
 *
 * The extraction function is the critical path — it determines what the LLM
 * sees and therefore the quality of the summary. It is a pure function so it
 * can be tested in isolation without any LLM calls.
 *
 * Event projection table (what each event type contributes to the basis):
 *   user_message          → "User: {content}"
 *   llm_response.text     → "Agent: {text}"
 *   tool_call             → buffered; paired with its tool_result
 *   tool_result (ok)      → "  {name} {arg} → ok"
 *   tool_result (error)   → "  {name} {arg} → error — {first line}"
 *   agent_error           → "Error: {message}"
 *   session_resumed       → only the summary field (carry-forward context section)
 *   turn_interrupted/err  → "[Turn interrupted due to error]"
 *   compacted             → "[Context compacted by server]"
 *   everything else       → dropped
 */

import type { OmegaEvent } from "./events.js";
import type { StreamProvider } from "./stream-provider.js";
import { primaryToolArg } from "./tools.schema.js";

// ---------------------------------------------------------------------------
// LLM instructions
// ---------------------------------------------------------------------------

/**
 * System prompt for the summarisation call.
 * Used as the system parameter so instructions "hit harder" than if they
 * were embedded in the user message.
 */
export const RESUMPTION_SUMMARY_INSTRUCTIONS = `\
Summarise the coding session history below so it can be continued in a new session.

Produce a concise summary (1000–2000 words) covering exactly what a developer \
needs to continue the work seamlessly:

1. **Current state** (snapshot, not narrative): which files were changed and \
how they currently stand, what constants/config values are set to, which plan \
items are done vs. pending.

2. **Next step**: the single most important thing to do next, as specifically \
as possible (exact file, function, or test name).

3. **Key decisions**: conclusions that should not be re-litigated — design \
choices made, approaches confirmed or rejected, and why.

4. **Learnings / what not to do**: anything tried that failed and why, so the \
same dead ends are not re-explored.

5. **Technical anchors**: specific file paths, function/type/constant names, \
commit hashes, and test names relevant to continuing the work.

You must wrap your summary in a <summary></summary> block.

Additionally, produce a one-line description (max 80 chars) of what the \
session accomplished, wrapped in a <description></description> block. \
This is used for display in the session picker — be specific and concrete, \
not vague. Example: "Added JWT auth middleware and login endpoint tests".\
`;

/**
 * System prompt for the auto-naming call.
 */
export const AUTO_NAME_INSTRUCTIONS = `\
Name this coding session in 2–4 lowercase words separated by spaces.

Rules:
- Lowercase only, words separated by spaces, no punctuation
- No articles (a/an/the), no filler words
- Describe the subject, not the process: prefer "jwt login endpoint" over \
"implement jwt login"
- Examples: "jwt login", "auth tests", "session resumption", "event schema"

Respond with ONLY the name — no explanation, no punctuation, nothing else.\
`;

/** The model used for auto-naming (generateSessionName). */
const RESUMPTION_MODEL = "claude-sonnet-4-6";

// ---------------------------------------------------------------------------
// Basis extraction helpers
// ---------------------------------------------------------------------------

/** Return the first non-empty line of a string, truncated to 120 chars. */
function firstMeaningfulLine(s: string): string {
  const line = s.split("\n").find(l => l.trim().length > 0) ?? s;
  return line.slice(0, 120).trim();
}

interface Turn {
  events: OmegaEvent[];
}

/**
 * Group a flat event list into turns.
 * A turn starts with `user_message` and ends with `turn_end` or
 * `turn_interrupted`. Events outside any turn are ignored.
 */
function groupIntoTurns(events: OmegaEvent[]): Turn[] {
  const turns: Turn[] = [];
  let current: OmegaEvent[] | null = null;

  for (const e of events) {
    if (e.type === "user_message") {
      current = [e];
      turns.push({ events: current });
    } else if (current !== null) {
      current.push(e);
      if (e.type === "turn_end" || e.type === "turn_interrupted") {
        current = null;
      }
    }
  }

  return turns;
}

/**
 * Project a single turn into a markdown string.
 * Tool calls are paired with their results by ID.
 */
function projectTurn(turn: Turn, index: number): string {
  const lines: string[] = [`### Turn ${index}`];

  // Buffer tool_call events keyed by ID so results can be paired with them.
  const toolCallMap = new Map<
    string,
    Extract<OmegaEvent, { type: "tool_call" }>
  >();

  for (const e of turn.events) {
    switch (e.type) {
      case "user_message":
        lines.push(`\nUser: ${e.content.trim()}`);
        break;

      case "llm_response":
        if (e.text) {
          lines.push(`\nAgent: ${e.text.trim()}`);
        }
        break;

      case "tool_call":
        toolCallMap.set(e.id, e);
        break;

      case "tool_result": {
        const call = toolCallMap.get(e.id);
        const toolName = e.name;
        const arg = call ? primaryToolArg(call.name, call.input) : "";
        const argPart = arg ? ` ${arg}` : "";
        const resultStr = e.isError
          ? `error — ${firstMeaningfulLine(e.output)}`
          : "ok";
        lines.push(`\n  ${toolName}${argPart} → ${resultStr}`);
        break;
      }

      case "agent_error":
        lines.push(`\nError: ${e.error}`);
        break;

      case "turn_interrupted":
        if (e.reason === "error") {
          lines.push("\n[Turn interrupted due to error]");
        }
        break;

      case "compacted":
        lines.push("\n[Context compacted by server]");
        break;

      default:
        // All other event types are dropped from the basis.
        break;
    }
  }

  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Public: basis extraction
// ---------------------------------------------------------------------------

/**
 * Extract the basis text from a session's event list.
 *
 * The basis is a markdown-formatted string structured for LLM readability:
 *
 *   ## Carried-forward context   (if a prior session_resumed event exists)
 *   <prior summary>
 *
 *   ## Session events
 *
 *   ### Turn 1
 *   User: ...
 *   Agent: ...
 *     tool arg → ok
 *
 *   ### Turn 2
 *   ...
 *
 * This is a pure function — no I/O, no LLM calls. Can be tested in isolation.
 */
export function extractResumptionBasis(events: OmegaEvent[]): string {
  const parts: string[] = [];

  // Find the last session_resumed event — its summary is the carry-forward
  // context from all prior sessions. Events before it are not re-processed.
  let resumedEventIdx = -1;
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i]!.type === "session_resumed") {
      resumedEventIdx = i;
      break;
    }
  }

  if (resumedEventIdx >= 0) {
    const resumedEvent = events[resumedEventIdx] as Extract<
      OmegaEvent,
      { type: "session_resumed" }
    >;
    if (resumedEvent.summary.trim()) {
      parts.push("## Carried-forward context\n\n" + resumedEvent.summary.trim());
    }
  }

  // Only process events after the last session_resumed (or all events if none).
  const relevantEvents = events.slice(resumedEventIdx + 1);
  const turns = groupIntoTurns(relevantEvents);

  if (turns.length > 0) {
    const turnStrings = turns.map((t, i) => projectTurn(t, i + 1));
    parts.push("## Session events\n\n" + turnStrings.join("\n\n"));
  }

  if (parts.length === 0) {
    return "(empty session — no turns recorded)";
  }

  return parts.join("\n\n");
}

// ---------------------------------------------------------------------------
// Public: summary extraction from LLM response
// ---------------------------------------------------------------------------

/**
 * Extract the summary text from an LLM response.
 * Parses the `<summary>...</summary>` block if present.
 * Falls back to the full response text if the block is absent.
 */
export function extractSummaryFromResponse(responseText: string): string {
  const match = responseText.match(/<summary>([\s\S]*?)<\/summary>/);
  if (match) return match[1]!.trim();
  return responseText.trim();
}

/**
 * Extract the description text from an LLM response.
 * Parses the `<description>...</description>` block if present.
 * Returns undefined if the block is absent.
 */
export function extractDescriptionFromResponse(responseText: string): string | undefined {
  const match = responseText.match(/<description>([\s\S]*?)<\/description>/);
  if (match) return match[1]!.trim().slice(0, 120); // hard cap
  return undefined;
}

// ---------------------------------------------------------------------------
// Public: auto-naming call
// ---------------------------------------------------------------------------

/**
 * Call the LLM to produce a short session name from the first user message
 * and the first agent response text. Uses the same StreamProvider as normal
 * turns — no separate provider abstraction.
 */
export async function generateSessionName(
  firstUserMessage: string,
  firstAgentResponse: string,
  provider: StreamProvider,
): Promise<string> {
  const userContent =
    `First user message: ${firstUserMessage.slice(0, 300).trim()}\n` +
    `First agent response: ${firstAgentResponse.slice(0, 400).trim()}`;
  const stream = provider({
    model: RESUMPTION_MODEL,
    max_tokens: 64,
    system: AUTO_NAME_INSTRUCTIONS,
    messages: [{ role: "user", content: userContent }],
  });
  // We only need the final text — no streaming needed for a short name.
  const message = await stream.finalMessage();
  const text = message.content
    .filter((b: any) => b.type === "text")
    .map((b: any) => (b as { type: "text"; text: string }).text)
    .join("");
  // Sanitise: lowercase, collapse whitespace, strip non-word chars
  return text
    .toLowerCase()
    .replace(/[^a-z0-9 ]/g, "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 60);
}
