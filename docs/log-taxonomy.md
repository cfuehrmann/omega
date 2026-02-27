# Log Taxonomy

> **Deprecated (Step 4 complete):** `src/logger.ts` and pino have been retired.
> This document is kept for historical reference only.
> The canonical event log is now `sessions/events.jsonl` — see `src/session-event.ts`.

Authoritative reference for all structured log entries that were written to `omega.log`
(via pino in `src/logger.ts`). Every entry had a `kind` field. There were
exactly two top-level kinds: **`message`** and **`infra`**.

---

## Top-level shape

```
{ kind, ... }
```

### `kind: "message"` — real sender→receiver events

A message is an observable act of communication between two named parties.

```
{ kind: "message", sender, receiver, message, ...fields }
```

| Field      | Type   | Values |
|------------|--------|--------|
| `kind`     | string | `"message"` |
| `sender`   | string | `"agent"`, `"user"`, `"llm"` |
| `receiver` | string | `"agent"`, `"user"`, `"llm"` |
| `message`  | string | see table below |

### `kind: "infra"` — internal bookkeeping events

An infra entry is not a communication between parties. It has no `sender` or
`receiver`. A separate `event` field names what happened.

```
{ kind: "infra", event, ...fields }
```

| Field   | Type   | Values |
|---------|--------|--------|
| `kind`  | string | `"infra"` |
| `event` | string | see table below |

---

## Message entries (`kind: "message"`)

Three actors: **agent** (Omega process), **user** (human operator), **llm**
(the upstream model API). Direction is always explicit from `sender`+`receiver`.

| sender  | receiver | message             | Meaning |
|---------|----------|---------------------|---------|
| `user`  | `agent`  | `call`              | Operator submits a prompt |
| `agent` | `llm`    | `call`              | Main agentic-loop API call |
| `llm`   | `agent`  | `response`          | LLM reply in the main loop |
| `agent` | `agent`  | `tool_call`         | Agent invokes a tool |
| `agent` | `agent`  | `tool_result`       | Tool returns its result |
| `agent` | `llm`    | `compact_turn`      | LLM call to compact a completed turn |
| `agent` | `llm`    | `compact_session`   | LLM call to fold session into world-state |

**Notes:**
- `tool_result` sender is `agent`, not the tool itself — the tool is not a
  distinct actor; the agent owns the call and the result.
- `compact_turn` and `compact_session` are separate from the main loop `call`
  because they use a different prompt and different bookkeeping.
- When the agent receives a `response` from the LLM and there are tool calls
  inside it, that single `response` is followed by one `tool_call` + one
  `tool_result` entry per tool (potentially concurrent via `Promise.all`).

---

## Infra entries (`kind: "infra"`)

Infra entries capture internal lifecycle and aggregate bookkeeping that does
not represent a communication between parties.

| event              | Meaning |
|--------------------|---------|
| `startup`          | Process started; initial config logged |
| `turn_end`         | Full turn completed; aggregate token counts and cost |
| `session_end`      | Session fold complete; world-state written |
| `oauth_refresh`    | OAuth token refreshed |
| `oauth_error`      | OAuth token refresh failed |
| `api_retry`        | Retrying an API call after rate-limit or transient error |
| `context_truncated`| History truncated due to token budget |
| `diagnostic_written` | Fatal-error snapshot written to `diagnosis/` |

**Notes:**
- `turn_end` is the per-turn aggregate: total token buckets (`new_input`,
  `cache_write`, `cache_read`, `output`), cost in USD, cache savings in USD,
  turn index, and model used. It is *not* a sender→receiver message.
- `session_end` is written after `foldCurrentSessionIntoWorldState()` completes.
- `api_retry` may fire multiple times per turn; it carries the retry delay and
  the reason (rate-limit, transient, etc.).

---

## Implementation status

The taxonomy above is the **design target**. Existing code does not yet fully
conform:

- Current pino calls use legacy names (`api_request`, `api_response`,
  `tool_exec`, `api_call`) — see backlog.md LOG-2 for the rename backlog.
- `kind` and `sender`/`receiver`/`message` fields are not yet present on log
  entries; they are emitted as flat objects with event-name strings.
- TypeScript enforcement (discriminated union) is not yet implemented.

When implementing LOG-2, use this document as the specification.

---

## Design decisions (recorded here to avoid re-litigating)

1. **Field named `message`** (not `action`, `type`, `verb`, `event`). Chosen
   because it names the act of communication, which is the semantics of the
   `kind: "message"` category.
2. **Field named `event`** on infra entries (not `name`, `type`). Chosen because
   infra entries *are* events (lifecycle moments), not messages.
3. **`tool_result` sender is `agent`**, not a separate `tool` actor. The tool
   is an implementation detail; the agent is responsible for both the call and
   the result.
4. **Infra entries omit `sender`/`receiver`** entirely (not set to `null` or
   `"system"`). A TypeScript discriminated union will enforce this at compile
   time when LOG-2 is implemented.
5. **`turn_end`** (not `turn_summary`) — chosen for consistency with the
   existing `AgentEvent` type where the per-turn bookkeeping event is already
   named `turn_end`.
