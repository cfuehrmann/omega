# Omega — Self-Improving AI Coding Agent

## Vision

A personal AI coding agent that:
- Is primarily for coding, but usable for general tasks
- Is private / single-user — configuration can be code
- Can **bootstrap itself**: iterate on its own codebase as fast as possible
- Is **self-observing**: exposes logs, metrics, API usage, and cost data so that
  AI (including itself) can analyze and improve the agent
- Teaches its operator the Anthropic API by making its own API usage transparent
- Can **operate itself** to test its own features

## Design Principles

1. **Tight iteration loop** — The agent modifies itself, tests, and reloads
   with minimal friction. Errors and backtracking are minimized.
2. **Observability first** — Structured logs, API call traces, token counts,
   cost estimates, latency measurements, and decision traces are first-class.
3. **API transparency** — Every request/response to the provider is visible.
   Token usage and estimated cost are always shown.
4. **Low latency UX** — Streaming responses from the first token. The UI must
   never block or feel sluggish.
5. **No premature lock-in** — Start with a terminal UI, but architecture must
   allow adding a web UI (browser-based coding) later.
6. **UI-technology-independent design** — The *logical layout* (what
   information goes where, how panes relate) is defined abstractly. The same
   layout specification drives both terminal and web renderers. If the layout
   should show a chat pane beside an observability pane, that's true regardless
   of whether it's rendered with Ink or with HTML.
7. **Self-testable** — From an early version onward, the agent must be able to
   drive itself (send inputs, observe outputs) for automated end-to-end
   testing. Features are pinned down by E2E tests so regressions are caught.
8. **Stable core, evolvable surface** — From a defined early milestone, the
   core (agent loop, provider adapter, tool dispatch) is stable enough that the
   agent can reliably fix itself if bugs arise. Surface features (UI, new
   tools) evolve freely on top.

## Decisions Made

### Language: TypeScript

TypeScript is the clear choice for this project:
- First-party Anthropic SDK (`@anthropic-ai/sdk`)
- No compilation step — fastest possible edit-run loop
- AI models produce high-quality TypeScript with low error rates
- Massive ecosystem for everything we'll need
- Same language for terminal UI (Ink) and future web UI

Runtime (Node / Deno / Bun) deferred to implementation start.

### UI Framework: Ink (React for the Terminal)

Ink is React rendered to the terminal via Yoga (Flexbox). It provides:
- **Component-based architecture** — same mental model as web React
- **Flexbox layout** — `<Box>`, padding, margin, flex, etc.
- **`<Static>`** — permanently rendered output above the live area (perfect for
  completed messages / logs)
- **`useInput`** hook — keyboard handling
- **Focus management** — built-in focus system for interactive elements
- **Streaming-friendly** — React state updates re-render instantly, so
  streaming tokens into state gives real-time display

Notable: **Claude Code itself uses Ink.** So does Gemini CLI, Shopify CLI,
Cloudflare Wrangler, and Prisma.

### API Interaction: Streaming SSE

Anthropic has no persistent sockets. Every interaction is an HTTP request.
With `"stream": true`, the response arrives as Server-Sent Events:

```
message_start → content_block_start → content_block_delta* → content_block_stop → message_delta → message_stop
```

The TypeScript SDK wraps this with `client.messages.stream()` which provides
`.text_stream` (async iterator of text chunks) and `.finalMessage()`.

- **User-facing interactions**: Always streaming (low latency)
- **Background/batch operations**: May use non-streaming for simplicity
- **Token usage**: Available in `message_delta` event and final `usage` object

## Testing Strategy

### The "Playwright for Consoles" Problem

The agent must be testable end-to-end. This means:

1. **Ink testing library** (`ink-testing-library`) — Renders Ink components
   in-memory (no real terminal needed). Provides:
   - `render(<App />)` — returns `lastFrame()`, `frames`, `stdin.write()`,
     `rerender()`, `unmount()`
   - Frame assertions: check what the terminal would display
   - Stdin simulation: send keystrokes and text programmatically

   This is our Playwright equivalent. Tests can:
   - Start the agent
   - Send a user message via `stdin.write()`
   - Assert on the rendered output via `lastFrame()`
   - Verify API calls were made (via a mock provider adapter)
   - Verify tool invocations happened

2. **Self-operation** — The agent itself can invoke its own CLI interface
   programmatically, acting as both operator and subject. This enables:
   - The agent testing its own features ("send this message, verify the
     response appears in the UI")
   - Regression testing after self-modification
   - Smoke-testing a new version before swapping it in

3. **Layered test strategy**:
   - **Unit tests**: Pure functions (message formatting, cost calculation,
     tool parsing)
   - **Component tests**: Ink components in isolation via ink-testing-library
   - **Integration tests**: Agent core + mock provider (no real API calls)
   - **E2E tests**: Full agent with real or recorded API responses
   - **Self-tests**: Agent drives itself through scenarios

### Test Infrastructure

- Test runner: Vitest (fast, native TypeScript, watch mode)
- Component testing: ink-testing-library
- API mocking: Record/replay of SSE streams, or mock provider adapter
- CI: Tests run on every self-modification before the new version is accepted

## UI Architecture

### Abstract Layout Model

The layout is defined as a **logical structure** independent of rendering
technology. This same structure drives both Ink (terminal) and a future web
renderer.

```
┌─ Layout ──────────────────────────────────────────────┐
│                                                       │
│  ┌─ Static Zone (scroll-off) ────────────────────┐   │
│  │ Completed messages, finished tool calls,       │   │
│  │ committed output — never re-rendered           │   │
│  └────────────────────────────────────────────────┘   │
│                                                       │
│  ┌─ Live Zone ────────────────────────────────────┐   │
│  │                                                │   │
│  │  ┌─ Main Pane ───────────────────┐             │   │
│  │  │ Current streaming response    │  ┌─ Side ─┐ │   │
│  │  │ Active tool execution         │  │ Tokens │ │   │
│  │  │ User input area               │  │ Cost   │ │   │
│  │  │                               │  │ Logs   │ │   │
│  │  │                               │  │ API    │ │   │
│  │  └───────────────────────────────┘  └────────┘ │   │
│  │                                                │   │
│  └────────────────────────────────────────────────┘   │
│                                                       │
│  ┌─ Status Bar ──────────────────────────────────┐   │
│  │ Model │ Tokens (in/out) │ Cost │ Latency      │   │
│  └────────────────────────────────────────────────┘   │
│                                                       │
└───────────────────────────────────────────────────────┘
```

**Key layout decisions:**
- **Static zone**: Uses Ink's `<Static>` for completed items. They scroll off
  the top and are never re-rendered. This is where finished conversation turns
  and completed tool outputs go.
- **Live zone**: The active, re-rendering area. Split into main pane (chat/
  interaction) and optional side pane (observability).
- **Side pane**: Collapsible. Shows token counts, cost, recent logs, API trace.
  Can be toggled with a keyboard shortcut.
- **Status bar**: Always-visible single line at the bottom. Persistent summary
  of model, session tokens, cost, latency.

This layout maps naturally to both terminal (Ink `<Box>` with Flexbox) and web
(HTML `<div>` with CSS Flexbox/Grid).

### Layout for Maximum Evolvability

The layout is data-driven: a layout descriptor (TypeScript object or config)
defines which panes exist, their relative sizes, and what content they show.
Adding a new pane (e.g., a file tree, a diff viewer) means adding an entry to
the layout descriptor. The renderer (Ink or web) reads the descriptor and
produces the appropriate components.

This means the UI can evolve rapidly without touching rendering code.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│              Layout Descriptor (abstract)            │
│  Defines panes, sizes, content bindings              │
├──────────────────┬──────────────────────────────────┤
│  Ink Renderer    │  (Future) Web Renderer            │
│  Terminal UI     │  Browser UI                       │
├──────────────────┴──────────────────────────────────┤
│                    Agent Core                        │
│  - Conversation loop                                 │
│  - Tool dispatch (file I/O, shell, search, etc.)     │
│  - Self-modification orchestration                   │
│  - Decision trace                                    │
├─────────────────────────────────────────────────────┤
│               Provider Adapter                       │
│  - Anthropic Messages API (streaming SSE)            │
│  - Request/response logging                          │
│  - Token counting & cost estimation                  │
│  - Cache control management                          │
├─────────────────────────────────────────────────────┤
│              Observability Layer                     │
│  - Structured log sink (file + in-memory ring)       │
│  - API call trace store                              │
│  - Metrics aggregation                               │
│  - Self-analysis hooks (agent can query its logs)    │
├─────────────────────────────────────────────────────┤
│              Test Harness                            │
│  - ink-testing-library integration                   │
│  - Mock provider adapter (record/replay)             │
│  - Self-operation interface                          │
│  - E2E scenario runner                               │
└─────────────────────────────────────────────────────┘
```

## Self-Modification Strategy

The agent needs to modify itself and validate the changes. The approach:

1. **Edit** — Agent writes changes to its own source files (tool use)
2. **Validate** — Agent runs its test suite against the modified code
3. **Spawn** — Agent starts a new process with the modified code
4. **Smoke test** — New process runs a self-test scenario
5. **Swap** — If all checks pass, the new process takes over; old one exits
6. **Rollback** — If any check fails, changes are reverted (git)

State (conversation history, config) is persisted to disk so it survives
process restarts.

Git is integral: every self-modification is a commit. Failed modifications
are reverted. The git log becomes an audit trail of the agent's evolution.

## Stable Core Contract

From M1 onward, the following interfaces are frozen (backwards-compatible
changes only):

- **Tool interface**: How tools are defined, dispatched, and return results
- **Provider adapter interface**: How the agent talks to the LLM
- **Observability hooks**: How logs and metrics are emitted
- **Self-operation interface**: How the agent drives itself for testing

This guarantees that even if the agent introduces a bug in surface code, it
retains the ability to:
1. Read its own source
2. Run its tests
3. Identify the failure
4. Fix the code
5. Validate the fix

## Milestones

### M0 — Minimal Viable Agent (bootstrap target)
- [ ] Project setup (TypeScript, Ink, Vitest)
- [ ] Send messages to Anthropic API with streaming
- [ ] Display streamed response in terminal (basic Ink UI)
- [ ] Log every API call with token counts
- [ ] Status bar showing model + token usage
- [ ] First E2E test using ink-testing-library

### M1 — Self-Improvement Loop (stable core target)
- [ ] Tools: read file, write file, run shell command, list files
- [ ] Agent can modify its own code
- [ ] Spawn-validate-swap self-modification flow
- [ ] Structured logging (JSON lines)
- [ ] Frozen core interfaces
- [ ] Test suite the agent runs on itself before accepting changes
- [ ] Cost/token tracking with session aggregation

### M2 — Rich Terminal UI
- [ ] Abstract layout descriptor driving Ink rendering
- [ ] Side pane with observability data (toggleable)
- [ ] Static zone for completed turns
- [ ] Conversation history persistence
- [ ] Keyboard shortcuts / command palette

### M3 — Coding Agent Features
- [ ] Project context awareness (file tree, git status)
- [ ] Intelligent file search (grep, glob)
- [ ] Multi-file editing with diff review
- [ ] Test execution and error analysis
- [ ] Agent self-tests its coding features

### M4 — Web UI
- [ ] Web renderer consuming the same layout descriptor
- [ ] HTTP server serving the agent backend
- [ ] Browser-based coding interface
- [ ] Shared state between terminal and web sessions

## Next Steps

1. **Choose runtime** (Node / Deno / Bun) — deferred to implementation start
2. **Set up project** — package.json, tsconfig, Ink, Vitest, Anthropic SDK
3. **Build M0** — minimal streaming chat with token logging
