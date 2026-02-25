/**
 * LLM-based world-state compaction for Omega.
 *
 * compactWorldState(priorWorldState, sessionHistory, provider)
 *   Folds a completed session into the persistent world state.
 *   Returns the new world state string (to be written to disk).
 *
 * Turn compaction (zone 2) has been removed — history now grows verbatim
 * and relies on prompt caching for token efficiency (manifest Step 2).
 */

import type { MessageParam } from "@anthropic-ai/sdk/resources/messages";
import type { StreamProvider } from "./agent.js";

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/** Serialise messages to a readable text block for the compaction prompt. */
function serialiseMessages(msgs: MessageParam[]): string {
  return msgs.map((m) => {
    const role = m.role.toUpperCase();
    if (typeof m.content === "string") return `${role}: ${m.content}`;
    if (!Array.isArray(m.content)) return `${role}: [unknown content]`;
    const parts = m.content.map((b: any) => {
      if (b.type === "text") return b.text;
      if (b.type === "tool_use") return `[tool_use: ${b.name}(${JSON.stringify(b.input)})]`;
      if (b.type === "tool_result") return `[tool_result: ${b.content}]`;
      return `[${b.type}]`;
    });
    return `${role}: ${parts.join("\n")}`;
  }).join("\n\n");
}

/** Call the LLM with a single user message and return the text response. */
async function callLlm(
  prompt: string,
  provider: StreamProvider,
  model = "claude-sonnet-4-6",
  maxTokens = 2048
): Promise<string> {
  const stream = await provider({
    model,
    max_tokens: maxTokens,
    system: "You are a context compactor. Respond only with the requested summary, no preamble.",
    tools: [],
    messages: [{ role: "user", content: prompt }],
  });
  const msg = await stream.finalMessage();
  const textBlock = msg.content.find((b: any) => b.type === "text");
  return textBlock ? (textBlock as any).text : "";
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Fold a completed session into the persistent world state.
 *
 * @param priorWorldState - The existing world state string (null if none exists yet).
 * @param sessionHistory - The full history of the completed session.
 * @param provider - The stream provider to use for the LLM call.
 * @returns The new world state string (caller should write this to disk).
 */
export async function compactWorldState(
  priorWorldState: string | null,
  sessionHistory: MessageParam[],
  provider: StreamProvider,
  model = "claude-sonnet-4-6"
): Promise<string> {
  const sessionText = serialiseMessages(sessionHistory);

  const priorSection = priorWorldState
    ? `Here is the current state of the world (from previous sessions):\n<world_state>\n${priorWorldState}\n</world_state>\n\n`
    : "";

  const prompt = `${priorSection}Here is the session that just ended:\n<session>\n${sessionText}\n</session>\n\nProduce an updated "state of the world" document that captures:
- The overall purpose and current state of the project
- Key architectural decisions and why they were made
- Important files and what they do
- What was accomplished in the most recent session (1–4 sentences max; omit commit hashes, step-by-step procedural detail, and anything already captured in the durable sections above — only record net outcomes and decisions that change how future sessions should behave)
- Open issues or known problems
- Anything the agent should know to continue working effectively

This document will be injected into the system prompt for the next session.
Write in present tense for current state, past tense for history.
Be concise but complete. Ruthlessly prune: prefer one accurate sentence over three redundant ones.
No preamble, just the document.`;

  return callLlm(prompt, provider, model, 4096);
}
