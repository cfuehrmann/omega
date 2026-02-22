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
UI (Ink)
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
  | { type: "api_call_start";
      callNumber: number;
      model: string;
      system: string;
      tools: Anthropic.Tool[];
      messages: Anthropic.MessageParam[] }            // snapshot before call
  | { type: "api_response";
      stopReason: string;                             // "end_turn" | "tool_use" | ...
      usage: { input_tokens: number; output_tokens: number };
      content: Anthropic.ContentBlock[] }             // text + tool_use blocks
  | { type: "tool_call";
      id: string;
      name: string;
      input: any;                                     // tool-specific, see below
      formatted: string }                             // human-readable summary
  | { type: "tool_result";
      id: string;
      name: string;
      formatted: string;
      result: ToolResult }
  | { type: "metrics";
      metrics: TurnMetrics;
      startedAt: string }                             // HH:MM:SS, per API call
  | { type: "turn_end";
      metrics: TurnMetrics;                           // aggregated across all calls
      toolCalls: string[] }                           // all tool names this turn
  | { type: "error";         error: string }
  | { type: "interrupted" }

type TurnMetrics = {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null;
  totalMs: number;
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
```

Tool results feed back into `messages` as `tool_result` blocks, which the
model receives on the next API call.

---

## The agentic loop in type terms

```
while true:
  request:  MessageCreateParams  →  Anthropic API
  response: Message              ←  Anthropic API

  if response.stop_reason == "end_turn":
    yield turn_end; break

  for each tool_use block in response.content:
    yield tool_call
    result = executeTool(name, input)   // string
    yield tool_result
    append tool_result to messages

  // messages now larger — next request carries more context
```

---

## What the UI shows

The UI renders each `AgentEvent` as a block in the log:

| Event | Colour | What you see |
|---|---|---|
| user prompt | green | separator + text |
| `api_call_start` | cyan | pseudo-JSON of request params |
| `api_response` | blue | pseudo-JSON of response |
| `tool_result` | yellow | formatted call + result preview |
| `turn_end` | dim | aggregated metrics |
| `error` | red | error message |
