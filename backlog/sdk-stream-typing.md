# Type the Anthropic beta SDK stream events instead of `any`

## Problem

`StreamProvider` discards the SDK's stream event types:

```typescript
// stream-provider.ts
export type StreamProvider = (
  params: Anthropic.Beta.Messages.MessageCreateParamsNonStreaming,
) => {
  [Symbol.asyncIterator](): AsyncIterator<any>;  // ← should be BetaRawMessageStreamEvent
  finalMessage(): Promise<Anthropic.Beta.Messages.BetaMessage>;
};
```

This `any` cascades through all stream processing in `streamLlmCall` (`agent.ts`).
Every `event.type`, `event.delta.type`, `event.delta.text`,
`event.content_block?.type` check is untyped — a typo would silently fail at
runtime rather than failing at compile time.

## Fix: `StreamProvider` iterator type

Replace `AsyncIterator<any>` with
`AsyncIterator<Anthropic.Beta.Messages.BetaRawMessageStreamEvent>`. The SDK's
`BetaMessageStream` already implements
`AsyncIterable<BetaRawMessageStreamEvent>`, so the real provider is already
compatible — only the type declaration is wrong.

Mock providers in tests will need to produce events that satisfy the type. This
is desirable: it forces test events to match the real SDK shape.

## Unnecessary `as any` casts on `BetaMessage` fields

Separately, several places in `agent.ts` cast to `any` to access fields that
are **already properly typed** on `BetaMessage` / `BetaUsage` in SDK 0.80.0:

| Location | Cast | Unnecessary because |
|---|---|---|
| `elideAnthropicResponse` | `(resp as any).context_management` | `BetaMessage.context_management` is `BetaContextManagementResponse \| null` |
| `streamLlmCall` (llm_response for resumption) | `(result.response.usage as any).service_tier` | `BetaUsage.service_tier` is `'standard' \| 'priority' \| 'batch' \| null` |
| `sendMessage` (applied_edits detection) | `(response as any).context_management?.applied_edits` | Same as above — `BetaMessage` has the field |

These casts were likely written before the SDK added the types, and now just
mask type information. Removing them lets the compiler verify field access.

## Scope

- `src/stream-provider.ts` — change the iterator type
- `src/agent.ts` — remove unnecessary `as any` casts, fix any resulting type
  errors
- Test files (`src/agent-integration.test.ts`, `src/agent-thinking.test.ts`,
  `src/context-hash.test.ts`, `src/system-prompt/system-prompt-append.test.ts`,
  etc.) — update mock stream events to satisfy the typed iterator
