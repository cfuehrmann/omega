# Data Flows — Types at Every Layer

The motivation: understanding a system means knowing what kinds of values
flow through its communication boundaries, and what shape those values have.
This document makes that explicit for Omega.

---

## Layers and their boundaries

```
You
 │  string (your prompt)
 ▼
Terminal UI (src/terminal/)
 │  AgentEvent stream (async generator)
 ▼
Agent Core
 │  Anthropic.MessageCreateParams (HTTP request body)
 ▼
Anthropic API
 │  Anthropic.Message (HTTP response body)
 ▼
Agent Core
 │  ToolCall / ToolResult (internal, per tool invocation)
 ▼
Tool Executors (read_file, run_command, etc.)
```

---

## Layer 1 — You → UI

A plain string. Nothing interesting here structurally.

---

## Layer 2 — UI → Agent: `AgentEvent`

The agent is an async generator. The UI iterates it with `for await`.
Every event has a `type` discriminant. Full union:

```typescript
type AgentEvent =
  | { type: "status";        message: string }
  | { type: "text";          text: string }          // streamed chunk
  | { type: "user_message";  content: string }       // echo of submitted prompt
  | { type: "llm_call";
      llmCallNumber: number;
      provider: "anthropic" | "openai";
      url: string;
      request: any }                                  // snapshot before call
  | { type: "llm_to_agent";
      provider: "anthropic" | "openai";
      url: string;
      stopReason: string;                             // "end_turn" | "tool_use" | ...
      usage: { input_tokens: number; output_tokens: number };
      content: Anthropic.ContentBlock[] }             // text + tool_use blocks
  | { type: "agent_to_agent_tool_call";
      id: string;
      name: string;
      input: any;                                     // tool-specific, see below
      formatted: string }                             // human-readable summary
  | { type: "agent_to_agent_tool_result";
      id: string;
      name: string;
      formatted: string;
      result: ToolResult }
  | { type: "tool_result_message" }                  // signals tool results assembled
  | { type: "metrics";
      metrics: TurnMetrics;
      startedAt: string }                             // HH:MM:SS, per API call
  | { type: "turn_end";
      metrics: TurnMetrics;                           // aggregated across all calls
      toolCalls: string[];                            // all tool names this turn
      provider: "anthropic" | "openai";
      model: string }
  | { type: "agent_error";   error: string }
  | { type: "llm_error";     error: string; attempt: number; willRetry: boolean }
  | { type: "turn_interrupted" }

type TurnMetrics = {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null;
  totalMs: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
  savedUsd?: number;
}

type ToolResult = {
  output: string;
  isError: boolean;
  durationMs: number;
}
```

---

## Layer 3 — Agent → Anthropic API: `MessageCreateParams`

What actually goes over the wire (simplified to what we use):

```typescript
{
  model: string,                  // e.g. "claude-sonnet-4-6"
  max_tokens: number,             // config.maxOutputTokens = 8192
  system: string,                 // system prompt (plain text)
  tools: Tool[],                  // see tool shapes below
  messages: MessageParam[],       // conversation history — grows each loop iteration
}

// Each message is one of:
type MessageParam =
  | { role: "user";      content: string | ContentBlockParam[] }
  | { role: "assistant"; content: string | ContentBlockParam[] }

// Content blocks within a message:
type ContentBlockParam =
  | { type: "text";        text: string }
  | { type: "tool_use";    id: string; name: string; input: object }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error?: boolean }

// Note: the UI shows messages as a count only ("messages: <N messages>")
// since the full content is visible in the scrollback.
```

**Context growth:** Every agentic loop iteration appends to `messages`:
- assistant turn: the model's response (text + tool_use blocks)
- user turn: the tool results (tool_result blocks)

This is why `~N tokens` grows across API calls within one user prompt.

**Non-destructive truncation:** `buildApiMessages(history, budget)` in `agent.ts`
produces an ephemeral, token-budget-capped view of `llmMessageLog` for each API
call. The canonical `llmMessageLog` is never mutated.

---

## Layer 4 — Anthropic API → Agent: `Message`

The response object:

```typescript
{
  id: string,
  model: string,
  role: "assistant",
  stop_reason: "end_turn" | "tool_use" | "max_tokens" | "stop_sequence",
  stop_sequence: string | null,
  usage: {
    input_tokens: number,
    output_tokens: number,
    cache_creation_input_tokens: number | null,
    cache_read_input_tokens: number | null,
  },
  content: ContentBlock[],
}

type ContentBlock =
  | { type: "text";     text: string }
  | { type: "tool_use"; id: string; name: string; input: object }
```

`stop_reason === "tool_use"` means the loop continues.
`stop_reason === "end_turn"` means the loop ends.

---

## Layer 5 — Tool inputs and outputs

Each tool has a typed input (enforced by the Anthropic tool schema) and
returns a plain string (the tool result content).

```typescript
// read_file
input:  { path: string; offset?: number; limit?: number }
output: string   // file contents, or error message

// write_file
input:  { path: string; content: string }
output: string   // "Wrote N bytes (M lines) to path"

// edit_file
input:  { path: string; old_text: string; new_text: string }
output: string   // "replaced N line(s) with M line(s)"

// list_files
input:  { path: string; recursive?: boolean }
output: string   // newline-separated file list

// run_command
input:  { command: string; timeout?: number }
output: string   // stdout + stderr, capped at 100KB

// web_search
input:  { query: string }
output: string   // abstract + top results with URLs and snippets

// fetch_url
input:  { url: string }
output: string   // HTML stripped to plain text, truncated at 8000 chars

// grep_files
input:  { pattern: string; path: string; file_glob?: string; case_sensitive?: boolean; max_results?: number; context_lines?: number }
output: string   // file:line:text matches, capped at max_results

// find_files
input:  { pattern: string; path: string; type?: string; hidden?: boolean; max_results?: number }
output: string   // matching paths

// run_background
input:  { command: string; cwd?: string }
output: string   // { pid, logFile } as JSON-like string

// kill_process
input:  { pid: number; signal?: string }
output: string   // status message
```

Tool results feed back into `messages` as `tool_result` blocks, which the
model receives on the next API call. All tool output is capped at
`MAX_TOOL_OUTPUT_CHARS = 100_000` before entering history.

---

## The agentic loop in type terms

```
while true:
  apiView = buildApiMessages(llmMessageLog, apiBudget)  // ephemeral, never mutates log
  request:  MessageCreateParams  →  Anthropic API
  response: Message              ←  Anthropic API

  if response.stop_reason == "end_turn":
    yield turn_end; break

  for each tool_use block in response.content (in parallel):
    yield agent_to_agent_tool_call
    result = executeTool(name, input)   // string, capped at 100KB
    yield agent_to_agent_tool_result
    append tool_result to messages

  // messages now larger — next request carries more context
```

---

## Session persistence

Every event is also appended to `sessions/events.jsonl` (via `src/session-event.ts`).
Every `MessageParam` sent to the LLM is appended to `sessions/context.jsonl`
(via `src/context-store.ts`) as a `ContextRecord` with `hash`, `ts`, `role`, `content`.
Both files are rotated to `.prev` variants on startup. Each `llm_call` event carries
`contextHashes: string[]` — the ordered hashes of every message in the view actually
sent, cross-referencing `context.jsonl` entries by their `hash` field.

---

## What the UI shows

The terminal UI renders each `AgentEvent` as a block in the log:

| Event | Colour | What you see |
|---|---|---|
| user prompt | green | separator + text |
| `llm_call` | cyan | pseudo-JSON of request params |
| `llm_to_agent` | blue | pseudo-JSON of response |
| `agent_to_agent_tool_result` | yellow | formatted call + result preview |
| `turn_end` | dim | aggregated metrics |
| `agent_error` / `llm_error` | red | error message |
