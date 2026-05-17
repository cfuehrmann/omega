# Omega Event Schema Reference

Post-SCHEMA-8 event grammar.  Covers the wire shape of every `OmegaEvent`
variant, the ephemeral `StreamSignal` grammar, the slot-assembly algorithm
that maps provider stream indices to `context.jsonl` content blocks, and
the HASH-1 contract that makes byte-equal context comparison feasible.

**Authoritative source:** `crates/omega-types/src/events.rs` for event
types; `crates/omega-types/src/stream_signal.rs` for `StreamSignal`.
This document is a reading guide; the Rust source always wins on ambiguity.

---

## Two Persistence Paths

Every `OmegaEvent` is both streamed to UI consumers over WebSocket *and*
appended to `.omega/sessions/<ts>/events.jsonl`.  A parallel path writes
`.omega/sessions/<ts>/context.jsonl` from the same streaming data:

```
Provider stream (per-block streaming accumulators)
    │
    ├──► context.jsonl  (ContentBlock[] in API order, signatures preserved)
    │       ▲ source of truth for next API call — replay-safe
    │
    └──► events.jsonl  (chronological log; UI/audit; NEVER fed to API)
            ├── LlmResponseStarted
            ├── ThinkingBlock × N      (one per completed thinking block)
            ├── TextBlock × M          (one per completed text block)
            ├── ToolUseBlock × K       (one per completed tool_use block)
            ├── LlmResponseEnded       (usage incl. iterations, stop_reason, hash)
            ├── ToolCall × K           (re-emitted post-response at dispatch)
            └── ToolResult × K         (per future completion)
```

Never confuse the paths.  `context.jsonl` is the conversation state sent to
the LLM; `events.jsonl` is an audit log for the UI.  Modifying the `events.jsonl`
grammar never affects what the model sees.

---

## JSON Encoding Conventions

Two naming conventions coexist in the schema — by design, not accident:

| Scope | Convention | Why |
|---|---|---|
| `OmegaEvent` discriminator (`"type"` field) | `snake_case` | Persisted `events.jsonl` uses snake_case discriminators |
| Outer struct fields on most event structs | `camelCase` | Matches the pre-SCHEMA-8 TypeScript persisted format |
| `LlmResponseUsage` fields | `snake_case` | Mirrors Anthropic API's wire format verbatim |
| `UsageIteration` fields | `snake_case` | Same: Anthropic wire format |

**Gotcha:** `LlmResponseEndedEvent` has `#[serde(rename_all = "camelCase")]`
on the outer struct, producing `stopReason`, `clearedToolUses`, `contextHash`,
etc.  But its nested `usage: LlmResponseUsage` field keeps `snake_case`
(`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, …) because
`LlmResponseUsage` has *no* `rename_all`.  Both conventions appear side-by-side
in the same JSON object:

```json
{
  "type": "llm_response_ended",
  "stopReason": "end_turn",
  "contextHash": "aabbccddeeff0011",
  "usage": {
    "input_tokens": 100,
    "output_tokens": 50
  }
}
```

**`Option` fields without `#[serde(default)]`:** several optional fields
(e.g. `ThinkingBlockEvent.signature`, `LlmRetryEvent.http_status`,
`LlmRetryEvent.error_body`) are marked `#[serde(skip_serializing_if =
"Option::is_none")]` but have *no* `#[serde(default)]`.  They still
deserialize correctly from JSON that omits the key because `Option<T>`'s
own serde impl treats a missing key as `None`.  The `default` attribute
is only needed for non-`Option` types.

**`LlmCallEvent.cache_breakpoint_index` is nullable, not optional:** this
field is *always* serialized — either as a number or as `null`.  It does
not carry `#[serde(skip_serializing_if)]`.

---

## OmegaEvent Variants (26 total)

```
SessionStarted     ServerStarted       ServerStopped
UserMessage        LlmCall
LlmResponseStarted LlmResponseEnded    LlmResponseDiscarded
TextBlock          ThinkingBlock        ToolUseBlock
ToolCall           ToolResult           TurnEnd
LlmError           AgentError           TurnInterrupted
LlmRetry           ModelChanged         EffortChanged
TransportError     ResumingSession      SessionResumed
PauseRequested     TurnPaused           TurnContinued
```

All 26 are valid `events.jsonl` lines and valid WebSocket frames.  The
`StreamSignal` variants (`Text`, `Thinking`, `*BlockComplete`) are
WebSocket-only — never written to `events.jsonl`.

---

## Per-Variant Reference

### Session / server lifecycle

| Variant | Key fields | Notes |
|---|---|---|
| `SessionStarted` | `sessionId`, `path`, `model`, `effort`, `systemPrompt`, `omegaCommit`, `agentTimeZone` | First event in every session.  `omegaCommit` defaults to `"unknown"` on deserialise when absent (backward compat).  `agentTimeZone` is the IANA name of the agent host's local time zone at session start (e.g. `"Europe/Berlin"`, `"UTC"`); used by the UI to render every event's UTC `time` as agent-host-local wall-clock time via `Intl.DateTimeFormat`.  Defaults to `"UTC"` on deserialise when absent (backward compat with sessions recorded before the field existed). |
| `ServerStarted` | `time` | Server process started. |
| `ServerStopped` | `time`, `outcome` (`"clean"` \| `"error"`), `reason?` | Server process stopped. |
| `ResumingSession` | `resumedFrom`, `name?`, `basis` | Emitted before the first `LlmCall` in a resumed session. |
| `SessionResumed` | `resumedFrom`, `summary` | Emitted when the seed summary has been accepted by the LLM. |

### Turn lifecycle

| Variant | Key fields | Notes |
|---|---|---|
| `UserMessage` | `time`, `content` | User-submitted message. |
| `LlmCall` | `url`, `model`, `contextHashes`, `cacheBreakpointIndex`, `requestBytes`, `requestSummary?` | `cacheBreakpointIndex` is always serialized (as `null` when absent). |
| `TurnEnd` | `time`, `metrics` | Aggregate per-turn token totals.  `metrics` uses camelCase (`inputTokens`, `outputTokens`, `cacheCreationTokens?`, `cacheReadTokens?`). |
| `TurnInterrupted` | `time`, `reason?` | `reason` is `"aborted"` or `"error"` when present. |
| `PauseRequested` | `time` | User requested a pause. |
| `TurnPaused` | `time` | Agent reached a clean seam and paused. |
| `TurnContinued` | `time`, `mode` | `mode` is `"manual"` or `"auto"`. |

### LLM response block grammar (SCHEMA-8)

This is the core of SCHEMA-8.  Every response stream is bracketed by
`LlmResponseStarted` / `LlmResponseEnded` (or `LlmResponseDiscarded`),
with zero or more content-block events between them.

#### `LlmResponseStartedEvent`

```json
{ "type": "llm_response_started", "time": "<ISO>" }
```

Opener emitted on the first signal from a freshly-started provider
stream within a turn iteration.  Always followed by exactly one of
`LlmResponseEnded` or `LlmResponseDiscarded`.

Source: `crates/omega-types/src/events.rs` → `LlmResponseStartedEvent`.

#### `LlmResponseEndedEvent`

```json
{
  "type": "llm_response_ended",
  "time": "<ISO>",
  "stopReason": "end_turn",
  "contextHash": "aabbccddeeff0011",
  "usage": {
    "input_tokens": 100,
    "output_tokens": 50,
    "cache_creation_input_tokens": 10,
    "iterations": [
      { "type": "compaction", "input_tokens": 80, "output_tokens": 0 },
      { "type": "message",    "input_tokens": 20, "output_tokens": 50 }
    ]
  },
  "clearedToolUses": null,
  "clearedInputTokens": null,
  "responseSummary": null
}
```

Successful close of a provider stream.  All outer fields are camelCase
(`stopReason`, `clearedToolUses`, `clearedInputTokens`, `contextHash`,
`responseSummary`); the nested `usage` object keeps Anthropic's
snake_case field names.

`contextHash` is an FK into `context.jsonl` for the assistant record
written for this response.  It is filled by the agent *after* writing
the context record, then the event is persisted.

`usage.iterations` is absent (`null` or missing key) on non-compacted
responses.  **Compaction detection**: check whether any entry in
`iterations` has `type == "compaction"`.  This replaces the former
`OmegaEvent::Compacted` variant (removed in SCHEMA-8 Phase 6.5).

`clearedToolUses` and `clearedInputTokens` record context-window
housekeeping (tool-result clearing); they are unrelated to compaction.

Source: `crates/omega-types/src/events.rs` → `LlmResponseEndedEvent`,
`LlmResponseUsage`, `UsageIteration`.

#### `LlmResponseDiscardedEvent`

```json
{ "type": "llm_response_discarded", "time": "<ISO>" }
```

Pure marker.  Closer for `LlmResponseStarted` when the response is
abandoned mid-stream.  Always immediately precedes `LlmRetry`,
`LlmError`, or `TurnInterrupted`.  Zero or more `partial: true` block
events always appear between `LlmResponseStarted` and
`LlmResponseDiscarded`.

Source: `crates/omega-types/src/events.rs` → `LlmResponseDiscardedEvent`.

#### `TextBlockEvent`

```json
{ "type": "text_block", "time": "<ISO>", "text": "…", "partial": false }
```

One text content block from a streamed assistant response.  Emitted at
the provider's `content_block_stop` for a `text` block.

`partial: true` means the block was cut off by abandonment.  The agent
only emits partial text blocks when the accumulated text is non-empty
(empty text slots are silently skipped at flush — see §Slot-assembly
invariants).

Source: `crates/omega-types/src/events.rs` → `TextBlockEvent`.

#### `ThinkingBlockEvent`

```json
{
  "type": "thinking_block",
  "time": "<ISO>",
  "thinking": "let me think…",
  "signature": "sig_abc…",
  "partial": false
}
```

One thinking (extended-reasoning) content block.

**Invariant:** `signature.is_none() iff partial == true`.  A
successfully completed thinking block always carries a signature (the
Anthropic-provided cryptographic token required when echoing the block
in a subsequent API call).  A partial block never has a signature
because it was not finalised by the provider.  Both fields are kept on
the wire to make consumer logic uniform — callers check `partial` rather
than testing `signature.is_some()`.

The `signature` field is `#[serde(skip_serializing_if = "Option::is_none")]`;
it is absent in the JSON for partial blocks.

Source: `crates/omega-types/src/events.rs` → `ThinkingBlockEvent`.

#### `ToolUseBlockEvent`

```json
{
  "type": "tool_use_block",
  "time": "<ISO>",
  "id": "toolu_xyz",
  "name": "read_file",
  "input": { "path": "foo.txt" },
  "partial": false
}
```

One `tool_use` content block from a streamed assistant response.  Emitted
at `content_block_stop` for a tool_use block.  When `partial: true`,
`input` may be malformed JSON (the stream was cut off before the input
accumulator was complete); the agent does not dispatch partial blocks.

Source: `crates/omega-types/src/events.rs` → `ToolUseBlockEvent`.

### Tool dispatch

| Variant | Key fields | Notes |
|---|---|---|
| `ToolCall` | `time`, `id`, `name`, `input`, `contextHash` | Emitted by the agent *after* `LlmResponseEnded`, one per non-partial `ToolUseBlock`, sequentially in index order, just before parallel dispatch.  `contextHash` is the FK of the assistant context record from the preceding `LlmResponseEnded`. |
| `ToolResult` | `time`, `id`, `name`, `isError`, `durationMs`, `output` | Emitted on completion of a dispatched tool call. |

### Error / retry

#### `LlmRetryEvent`

```json
{
  "type": "llm_retry",
  "time": "<ISO>",
  "attempt": 1,
  "httpStatus": 429,
  "waitMs": 5000,
  "error": "rate limited",
  "retryAt": null,
  "errorBody": null,
  "reason": "retry-after"
}
```

Fields `text_fragment` and `thinking_fragment` were present in earlier
versions and removed in SCHEMA-8 Phase 6.5c.  Retry is now a pure
control event.  The partial content that streamed before the error is
captured in `partial: true` block events that precede the
`LlmResponseDiscarded` that precedes this event.

`reason` is `"retry-after"` when the provider sent a `Retry-After` header;
absent for ordinary policy-driven retries.

Source: `crates/omega-types/src/events.rs` → `LlmRetryEvent`.

| Variant | Key fields | Notes |
|---|---|---|
| `LlmError` | `url`, `error`, `httpStatus?` | Non-retryable provider call error. |
| `AgentError` | `error` | Generic agent-level error. |

### Miscellaneous

| Variant | Key fields | Notes |
|---|---|---|
| `ModelChanged` | `model` | Operator switched the active model. |
| `EffortChanged` | `effort` | Operator changed the thinking effort level. |
| `TransportError` | `error`, `context?` | Transport-layer error from the web server. |

---

## StreamSignal Grammar

`StreamSignal` variants are ephemeral — yielded by the agent loop to drive
live rendering in the UI.  **Never written to `events.jsonl`.**

```
Text              { index: usize, text: String }
Thinking          { index: usize, text: String }
TextBlockComplete      { index: usize, text: String }
ThinkingBlockComplete  { index: usize, signature: String }
ToolUseBlockComplete   { index: usize, id: String, name: String, input: Value }
```

`index` matches Anthropic's `content_block_start.index`.  Every delta
(`Text`, `Thinking`) and every block-complete signal carries the same
`index` so the agent's order-preserving accumulator can route the fragment
to the correct slot.

The `*BlockComplete` signals are consumed by the agent and absorbed into
`OmegaEvent::TextBlock` / `ThinkingBlock` / `ToolUseBlock` events.  They
are not forwarded to the WebSocket.

Source: `crates/omega-types/src/stream_signal.rs`.

---

## Slot-Assembly Invariants

The agent maintains a `BTreeMap<usize, BlockSlot>` keyed by the API's
`content_block_start.index`.  This replaces the legacy flat accumulators
(`text_buf`, `current_thinking`, etc.) that grouped blocks by kind and
silently reordered interleaved streams.

```rust
enum BlockSlot {
    Text    { text: String, sealed: bool },
    Thinking { thinking: String, signature: Option<String>, sealed: bool },
    ToolUse  { id: String, name: String, input: Value, sealed: bool },
}
```

**Routing:** `Text`/`Thinking` delta signals are appended to the slot at
their `index`, creating the slot if it does not yet exist.

**Sealing:** on a `*BlockComplete` signal the slot is marked `sealed: true`
and the corresponding `OmegaEvent::TextBlock` / `ThinkingBlock` /
`ToolUseBlock` is emitted.

**Assembly order:** `context.jsonl` assistant content blocks are produced by
iterating the `BTreeMap` in key (index) order — so they are in the same
order as the provider's `content_block_start.index` sequence, not grouped by
kind.  This is the only correct order when extended thinking is in use.

**Empty-text-slot skip:** empty `Text` slots (text accumulated to `""`) are
skipped when building `context.jsonl`.  This keeps the assistant context
record clean when a provider opens a text block and immediately closes it
without any deltas.  Non-empty text blocks and all thinking/tool-use blocks
are always included.

**Abandonment flush:** when a mid-stream abandonment fires
(`LlmRetry`/`LlmError`/`TurnInterrupted` while `response_started` is
true), the agent calls `make_abandonment_closers` (see
`crates/omega-agent/src/agent.rs`) which:

1. Iterates slots in index order.
2. Emits `partial: true` block events for every *unsealed* slot that has
   accumulated non-empty content (empty text slots are skipped; sealed
   slots have already been emitted as `partial: false` and are skipped).
3. Emits `LlmResponseDiscarded`.
4. Clears the slot map so the next attempt starts fresh.

The sealing flag is the reason a `TextBlockComplete` signal that arrives
*before* the abandonment produces a `partial: false` block event even though
the surrounding response was abandoned.  SCHEMA-8's invariant is that
every `LlmResponseStarted` is closed by exactly one of `LlmResponseEnded`
or `LlmResponseDiscarded` — not that every block in an abandoned response
must be partial.

Source: `crates/omega-agent/src/agent.rs` (module doc + `make_abandonment_closers`).
Executable spec: `crates/omega-e2e/tests/10_append_only.rs` (T5).

---

## HASH-1 Contract

`ContextHash` is a 16-character lowercase hex string — the first 8 bytes of
SHA-256 over the canonical encoding of `(role, content)`:

```text
sha256(serde_json::to_vec(&(role, content)))[..8]  →  16 lower-hex chars
```

where `role: &Role` and `content: &[ContentBlock]`.  The canonical form is
the default `serde_json` output with no pretty-printing, no key sorting (the
JSON `Object` preserves declaration order), and UTF-8 encoding.

**Why byte-equal `context.jsonl` is feasible:** after HASH-1 every
`ContextHash` is deterministic from the message content.  The only
non-deterministic field in a `context.jsonl` record is `time`.  The Phase 0
golden tests scrub `time` to `"<scrubbed>"` before comparison, making the
rest of the file byte-for-byte reproducible across runs.

**Breaking change boundary:** any change to `Role` or `ContentBlock` that
affects their `serde` output (field order, variant order, `#[serde(rename)]`,
`#[serde(skip_serializing_if)]`, addition of a field or variant) invalidates
every previously-saved session's hashes.  SCHEMA-8 does not touch these
types.

Source: `crates/omega-store/src/context_hash.rs` (module doc + lockdown
tests).  Golden harness: `crates/omega-agent/tests/goldens.rs`.

---

## Append-Only DOM Invariant and Abandonment Semantics

**Invariant:** once an `EventBlock` element with a given `data-block-id`
is rendered in the feed, that id must remain present in the DOM for the
remainder of the session.  The `events` Vec in the Leptos store is
append-only; blocks are keyed by their Vec index (`data-block-id`); no
block is ever removed or relocated.

This means an abandoned response leaves visible traces:

1. `LlmResponseStarted` — emitted when the first signal arrives.
2. Zero or more `partial: true` block events — content accumulated before
   the abandonment.  These render with a "Discarded — …" header and
   greyed/struck-through body.
3. `LlmResponseDiscarded` — the explicit closer, rendered as a small
   marker.
4. (On retry) fresh `LlmResponseStarted` and new block events for the
   retried response.

The pre-discard partial blocks remain visible after the retry completes.
The operator can always see what the assistant was saying when the network
blipped.

**Why this design:** it is the invariant that distinguishes *abandon + retry*
from *rewind*.  It also means that replaying `events.jsonl` from disk always
reproduces the same feed layout as the live-streamed session — guaranteed by
construction, verified by the T6 browser-refresh e2e test
(`crates/omega-e2e/tests/09_refresh.rs`).

Executable spec for the invariant: `crates/omega-e2e/tests/10_append_only.rs`
(T5 — records `data-block-id` sets after every injected WS frame and asserts
monotonically non-decreasing).

---

## Full Event Sequence Grammar

```
SessionStarted → (ServerStarted? → … → ServerStopped?)

UserMessage
  → (LlmCall
      → LlmResponseStarted
        → (TextBlock | ThinkingBlock | ToolUseBlock)*    // API index order
        → (LlmResponseEnded | LlmResponseDiscarded)
      → ToolCall*       // only after LlmResponseEnded; one per non-partial ToolUseBlock
      → LlmRetry?       // only after LlmResponseDiscarded
    )*
  → ToolResult*         // one per ToolCall, completion order
  → TurnEnd
```

Key invariants:

- Every `LlmResponseStarted` is followed by **exactly one** of
  `LlmResponseEnded` or `LlmResponseDiscarded`.
- `LlmResponseDiscarded` is followed by `LlmRetry` (retry path),
  `LlmError` (giving up), or `TurnInterrupted` (user aborted).
- `ToolCall` events only appear after `LlmResponseEnded` — never after
  `LlmResponseDiscarded`.
- Each `ToolCall` corresponds 1:1 to a non-partial `ToolUseBlock` from
  the preceding `LlmResponseEnded`'s response, matched by `id`.
- For `ThinkingBlock`: `signature.is_none() iff partial == true`.
- `partial: true` block events always immediately precede the
  `LlmResponseDiscarded` that closes their response.
- Content-block events appear in `content_block_start.index` order
  within each response bracket.
