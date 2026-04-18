import type {
  BetaMessageStreamParams,
  BetaMessage,
  BetaRawMessageStreamEvent,
} from "@anthropic-ai/sdk/resources/beta/messages/messages.js";

/**
 * A CreateMessageStream calls the LLM streaming API (or a mock in tests) and
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
export type CreateMessageStream = (
  params: BetaMessageStreamParams,
) => {
  [Symbol.asyncIterator](): AsyncIterator<BetaRawMessageStreamEvent>;
  finalMessage(): Promise<BetaMessage>;
};
