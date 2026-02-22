/**
 * LLM-based context compaction for Omega.
 *
 * Two compaction operations:
 *
 * 1. compactTurn(turn, previousSummary, provider)
 *    Summarises a completed turn (zone 2 entry) into a 2-message synthetic
 *    exchange: { role:"user", content:"[summary] ..." } + { role:"assistant", content:"Understood." }
 *    If previousSummary is given, the new summary folds it in.
 *
 * 2. compactWorldState(priorWorldState, sessionHistory, provider)
 *    Folds a completed session into the persistent world state.
 *    Returns the new world state string (to be written to disk).
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
async function callLlm(prompt: string, provider: StreamProvider): Promise<string> {
  const stream = await provider({
    model: "claude-sonnet-4-6", // ignored by mock providers in tests
    max_tokens: 2048,
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
 * Compact a completed turn into a 2-message synthetic exchange.
 *
 * @param turn - The raw messages for this turn (user prompt + all tool loops + final answer).
 * @param previousSummary - The current zone 2 summary (null if this is the first turn).
 * @param provider - The stream provider to use for the LLM call.
 * @returns A 2-element array: [synthetic user summary message, assistant ack].
 */
export async function compactTurn(
  turn: MessageParam[],
  previousSummary: string | null,
  provider: StreamProvider
): Promise<MessageParam[]> {
  const turnText = serialiseMessages(turn);

  const priorSection = previousSummary
    ? `Here is the existing session summary so far:\n<existing_summary>\n${previousSummary}\n</existing_summary>\n\n`
    : "";

  const prompt = `${priorSection}Here is the most recent conversation turn to fold in:\n<turn>\n${turnText}\n</turn>\n\nProduce an updated session summary that captures:
- What the user asked for
- What tools were called and what they found (briefly)
- What decisions were made or conclusions reached
- Any files created, modified, or deleted
- Any errors encountered and how they were resolved

Write in past tense, be concise. No preamble, just the summary text.`;

  const summary = await callLlm(prompt, provider);

  return [
    { role: "user", content: `[session summary up to this point]\n${summary}` },
    { role: "assistant", content: "Understood, I have the session context." },
  ];
}

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
  provider: StreamProvider
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

  return callLlm(prompt, provider);
}
