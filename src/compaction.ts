/**
 * LLM-based in-session compaction.
 *
 * compactHistory() summarises the head of the in-memory context and keeps the
 * last KEEP_RECENT_TURNS message-pairs verbatim. Called by the agent on
 * /compact commands and auto-compact threshold crossings.
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

export interface CompactionUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

/** Call the LLM with a single user message and return the text response and usage. */
async function callLlm(
  prompt: string,
  provider: StreamProvider,
  model = "claude-sonnet-4-6",
  maxTokens = 2048
): Promise<{ text: string; usage: CompactionUsage }> {
  const stream = await provider({
    model,
    max_tokens: maxTokens,
    system: "You are a context compactor. Respond only with the requested summary, no preamble.",
    tools: [],
    messages: [{ role: "user", content: prompt }],
  });
  const msg = await stream.finalMessage();
  const textBlock = msg.content.find((b: any) => b.type === "text");
  const text = textBlock ? (textBlock as any).text : "";
  const usage: CompactionUsage = {
    input_tokens:                  msg.usage.input_tokens,
    output_tokens:                 msg.usage.output_tokens,
    cache_creation_input_tokens:   (msg.usage as any).cache_creation_input_tokens ?? undefined,
    cache_read_input_tokens:       (msg.usage as any).cache_read_input_tokens ?? undefined,
  };
  return { text, usage };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Number of message-pairs (user + assistant) to keep verbatim at the tail. */
export const KEEP_RECENT_TURNS = 10;

/**
 * Automatic compaction threshold: trigger compaction when the last observed
 * prompt token count exceeds this value. Prompt tokens = input_tokens +
 * cache_read_input_tokens + cache_creation_input_tokens — all three categories
 * occupy the context window regardless of cache status.
 *
 * Set to 100,000 tokens (≈50% of Claude's 200k window), giving a large safety
 * margin before overflow while still compacting proactively.
 *
 * After compaction: 1 synthetic message + KEEP_RECENT_TURNS*2 = 21 messages.
 */
export const AUTO_COMPACT_THRESHOLD = 100_000;

/**
 * Compact the in-memory history by summarising the head and keeping the tail.
 *
 * Returns a new (shorter) history array:
 *   [ syntheticUserSummary, ...tail ]
 *
 * If history is short enough that there is nothing to compact (≤ KEEP_RECENT_TURNS
 * message-pairs), returns the original array unchanged.
 *
 * @param history   The full `MessageParam[]` history array.
 * @param provider  Stream provider for the LLM call.
 * @param model     Model to use for summarisation.
 * @returns         New (shorter) history array, plus counts for UI feedback.
 */
export async function compactHistory(
  history: MessageParam[],
  provider: StreamProvider,
  model = "claude-sonnet-4-6"
): Promise<{
  history: MessageParam[];
  syntheticMessage: MessageParam;
  tailStartIndex: number;
  originalCount: number;
  newCount: number;
  usage: CompactionUsage;
}> {
  const originalCount = history.length;

  // Keep the last KEEP_RECENT_TURNS complete message-pairs (user + assistant).
  // Each pair is 2 messages, so tailLength = KEEP_RECENT_TURNS * 2.
  const tailLength = KEEP_RECENT_TURNS * 2;

  // tailStartIndex: index into the original history where the tail begins.
  // If history is short, tail starts at 0 (the entire history is the "tail").
  const tailStartIndex = Math.max(0, originalCount - tailLength);

  if (originalCount <= tailLength) {
    // Nothing to compact — history is already short enough.
    const noopSynthetic: MessageParam = {
      role: "user",
      content: `[Compacted context summary: (nothing to compact)]`,
    };
    const zeroUsage: CompactionUsage = { input_tokens: 0, output_tokens: 0 };
    return { history, syntheticMessage: noopSynthetic, tailStartIndex: 0, originalCount, newCount: originalCount, usage: zeroUsage };
  }

  const head = history.slice(0, tailStartIndex);
  const tail = history.slice(tailStartIndex);

  const headText = serialiseMessages(head);

  const prompt =
    `Below is a portion of a conversation between an AI coding agent and the user. ` +
    `Summarise what happened: what the user asked for, what the agent did (key tool calls and their outcomes), ` +
    `what decisions were made, and what the resulting state is. ` +
    `Be concise but complete — the summary will replace these messages as context for the agent going forward.\n\n` +
    `<conversation>\n${headText}\n</conversation>\n\n` +
    `Write a dense, factual summary in plain prose. No preamble.`;

  const { text: summary, usage } = await callLlm(prompt, provider, model);

  const syntheticMessage: MessageParam = {
    role: "user",
    content: `[Compacted context summary: ${summary}]`,
  };

  const newHistory: MessageParam[] = [syntheticMessage, ...tail];
  return { history: newHistory, syntheticMessage, tailStartIndex, originalCount, newCount: newHistory.length, usage };
}


