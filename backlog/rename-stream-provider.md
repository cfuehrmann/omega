# Rename `StreamProvider` and fix its param type

## Current state

```typescript
// src/stream-provider.ts
export type StreamProvider = (
  params: Anthropic.Beta.Messages.MessageCreateParamsNonStreaming,
) => {
  [Symbol.asyncIterator](): AsyncIterator<any>;
  finalMessage(): Promise<Anthropic.Beta.Messages.BetaMessage>;
};
```

Two problems:

### 1. Wrong param type

`MessageCreateParamsNonStreaming` = `MessageCreateParamsBase` + `stream?: false`.
The SDK's `.stream()` method accepts `BetaMessageStreamParams` = `MessageCreateParamsBase`
(no `stream` field at all — streaming is implied by the method, not the params).

Using `NonStreaming` params for a function that creates streams is a misnomer.
It works structurally (the extra `stream?: false` is just ignored), but it
actively misleads anyone reading the type.

### 2. `StreamProvider` is the wrong name

"Provider" is a design-pattern word (DI injection), not a domain word. It hides
two things:

- **It's a function**, not an object. Reads like something with a `.getStream()`
  method.
- **It calls the LLM.** The name doesn't convey that this is the thing that
  actually contacts Anthropic. The DI-seam role is already clear from context
  (tests inject a mock, production uses the real thing).

The essential nature: **a function that takes message params and returns a
streaming response handle.**

## Target state

```typescript
// src/stream-provider.ts
export type CreateMessageStream = (
  params: Anthropic.Beta.Messages.BetaMessageStreamParams,
) => {
  [Symbol.asyncIterator](): AsyncIterator<any>;
  finalMessage(): Promise<Anthropic.Beta.Messages.BetaMessage>;
};
```

The `AsyncIterator<any>` should become `AsyncIterator<BetaRawMessageStreamEvent>`
as part of the stream-event typing work (see `backlog/type-sdk-stream-events.md`),
but that's a separate change.

## Scope

- Rename `StreamProvider` → `CreateMessageStream` everywhere
- Change param type from `MessageCreateParamsNonStreaming` to `BetaMessageStreamParams`
- Update the CLAUDE.md mention ("If `StreamProvider` is renamed, update this
  file too")
- Rename `src/stream-provider.ts` → `src/create-message-stream.ts`
- Update `makeDefaultStreamProvider` → `makeDefaultCreateMessageStream` (or a
  better name — the factory's name should follow naturally from the type name)
