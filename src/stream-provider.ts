import type Anthropic from "@anthropic-ai/sdk";

/**
 * A StreamProvider calls the LLM streaming API (or a mock in tests) and
 * returns an object with an async iterator of raw stream events and a
 * `finalMessage()` method.
 *
 * The return type mirrors the real Anthropic SDK:
 * `client.beta.messages.stream()` returns a BetaMessageStream synchronously.
 *
 * This type is shared by `Agent` (for normal turns) and `session-resume.ts`
 * (for auto-naming) to avoid a circular import.
 *
 * NOTE: This type is referenced by name in .omega/system-prompt-append.md.
 * If you rename it, update that file too.
 */
export type StreamProvider = (
  params: Anthropic.Beta.Messages.MessageCreateParamsNonStreaming,
) => {
  [Symbol.asyncIterator](): AsyncIterator<any>;
  finalMessage(): Promise<Anthropic.Beta.Messages.BetaMessage>;
};
