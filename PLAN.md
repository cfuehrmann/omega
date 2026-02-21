# Omega — Self-Improving AI Coding Agent

*In the spirit of the [suckless philosophy](https://suckless.org/philosophy/):
minimal, clear, frugal. Configuration is code. Complexity is removed, not
managed. The tool does what you need and nothing else.*

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
2. **Terminal-native** — The agent's primary UI is Ink (React for the
   terminal). The agent can see its own rendered output as text, run its
   tests, and verify results — all without leaving the terminal. This makes
   the agent the fastest possible self-iterator. A browser UI comes later
   for richer human experience.
3. **Observability first** — Structured logs, API call traces, token counts,
   cost estimates, latency measurements, and decision traces are first-class.
4. **API transparency** — Every request/response to the provider is visible.
   Token usage and estimated cost are always shown.
5. **Low latency UX** — Streaming responses from the first token. The UI must
   never block or feel sluggish.
6. **UI-technology-independent design** — The *logical layout* (what
   information goes where, how panes relate) is defined abstractly. The same
   layout specification drives any renderer. If the layout should show a chat
   pane beside an observability pane, that's true regardless of whether it's
   rendered with Ink in a terminal or React in a browser.
7. **Self-testable** — From an early version onward, the agent must be able to
   drive itself (send inputs, observe outputs) for automated end-to-end
   testing. Features are pinned down by E2E tests so regressions are caught.
8. **Stable core, evolvable surface** — From a defined early milestone, the
   core (agent loop, provider adapter, tool dispatch) is stable enough that the
   agent can reliably fix itself if bugs arise. Surface features (UI, new
   tools) evolve freely on top.
9. **Full machine access** — The agent can use anything available on the Linux
   machine. The only boundary is `sudo`: when root privileges are needed, the
   agent asks the operator to either run the command themselves or enter the
   password. Everything else is fair game.
10. **Graduated trust** — In early phases, the agent asks the operator to
    confirm commands before executing them. As trust is established, the
    operator can widen the auto-approve policy until only destructive or
    privileged operations require confirmation.
11. **Full payload visibility** — The operator can see exactly what goes to
    the model on every call: system prompt, conversation history, tool
    definitions, cached prefixes — everything. This is shown in the UI as a
    collapsible section (collapsed by default) with a byte/token size count.
    No hidden magic. If the system prompt changes, the operator notices.
12. **Session handoff** — Sessions are designed so work can be handed off to
    a different model or a new session when limits are reached. The planning
    document and structured session state are the handoff mechanism. The
    incoming model reads the plan and picks up where the previous left off.
13. **Helix-style interaction** — The UI uses modal editing inspired by the
    Helix editor: a **normal mode** for navigation and commands, and an
    **insert mode** for text input. Mouse support is included. Details of the
    keymap are deferred, but the architecture assumes modal input from the
    start.

## Decisions Made

### Language: TypeScript

TypeScript is the clear choice for this project:
- First-party Anthropic SDK (`@anthropic-ai/sdk`)
- No compilation step — fastest possible edit-run loop
- AI models produce high-quality TypeScript with low error rates
- Massive ecosystem for everything we'll need
- Same language and React component model for terminal UI and future browser UI

### Runtime: Bun

Bun is the best fit for a self-improving agent:
- Native TypeScript execution — no compile step, no `tsx` wrapper
- Fastest startup (~5x faster than Node+tsx) — critical when the agent
  restarts itself on every self-modification
- Built-in test runner (can replace Vitest if desired)
- Full compatibility with the Anthropic SDK
- Ink 6.x + ink-testing-library 4.x confirmed working (smoke-tested)
- npm-compatible — full ecosystem access

**Verified compatibility** (smoke test 2025-02-21):
- Ink rendering, flexbox layout, borders, text styling ✅
- `<Static>` zone ✅
- State updates / streaming simulation ✅
- `ink-testing-library` render + assert ✅
- `useInput` with TTY ✅
- Clean exit ✅

### UI Strategy: Terminal-First (Ink), Browser Later (Vite + React)

**Terminal-first for fastest agent iteration:**
- The agent's tools are all terminal-based (read files, write files, run
  commands). Terminal output is its native medium.
- Ink renders React components as text — the agent can literally read what
  its own UI looks like by running it.
- `ink-testing-library` lets the agent render components and inspect output
  as strings — no browser, no screenshots, no Playwright process.
- Errors, stack traces, React errors — all directly readable in stdout/stderr.
- Single process. No browser to launch, coordinate, or debug through.
- Edit → run → see output. The tightest possible loop for a text-native AI.

**Browser later for richer human experience:**
- Vite + React will be added as a second renderer
- Same React component model, same hooks, same layout descriptor
- The abstract layout ensures the browser UI is a bounded implementation task,
  not a rearchitecting effort
- Browser DevTools, visual layout inspection, etc. benefit the human operator

**Why terminal-first over browser-first?**
The primary developer of this agent is the agent itself. Browser DevTools are
excellent for humans but invisible to the model. The agent cannot see hot
reloads, inspect DOM elements, or use React DevTools. In the terminal, the
agent can see everything. Optimizing for the agent's iteration speed means
terminal-first.

**Architecture implication:**
```
Agent Core (pure TypeScript — no framework imports)
    ↓ exposes
Agent API (function calls, events, state)
    ↓ consumed by
├── Terminal Renderer (Ink)             ← M0
└── Browser Renderer (Vite + React)    ← future
```

The agent core must never import from `react`, `ink`, or `vite`. Renderers
are thin consumers of the core API. Shared React hooks (like
`useStreamingMessage()`) can live in a shared package used by both renderers.

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

## Tool Strategy

### Full Machine Access

The agent has unrestricted access to the Linux machine as the current user.
It can:
- Read, write, and delete any file the user owns
- Run any command (compilers, package managers, git, curl, docker, systemctl,
  etc.)
- Install packages, manage services, configure the system
- Access the network (APIs, downloads, SSH to other machines)
- Use any tool installed or installable on the machine

### Web Search

The agent can search the web — look up documentation, error messages, API
references, package versions, etc. This is distinct from fetching a known URL
(which is just `curl`).

Implementation: a lightweight search tool that queries a search engine
(DuckDuckGo instant answers API, or similar — no API key required) and
returns a list of results (title, URL, snippet). The agent can then fetch
specific URLs with `curl` or a readability-mode fetcher that extracts article
text from HTML.

This is a first-class tool, not an afterthought. An agent that can't search
is guessing when it could be looking things up.

### The sudo Boundary

The only hard boundary is privilege escalation. When a command requires `sudo`:

1. **Agent recognizes** it needs root (or the command fails with permission
   denied)
2. **Agent asks the operator** with one of:
   - "This needs sudo. Please run: `sudo ...`" (operator does it themselves)
   - "This needs sudo. Enter your password and I'll run it" (agent runs
     `sudo` with the password provided via secure input)
3. **Agent never stores** the sudo password beyond the immediate use

### Graduated Trust Model

Command execution has a trust level that the operator controls:

```typescript
type TrustLevel = 'confirm-all' | 'confirm-destructive' | 'auto'

interface TrustPolicy {
  level: TrustLevel
  // Patterns that always require confirmation regardless of trust level
  alwaysConfirm: string[]   // e.g., ['rm -rf', 'sudo', 'reboot', 'dd ']
  // Patterns that are always auto-approved regardless of trust level
  alwaysAllow: string[]     // e.g., ['ls', 'cat', 'git status', 'grep']
}
```

**`confirm-all`** (default, early phase):
Every command is shown to the operator before execution. The operator sees
exactly what will run and approves or rejects it. This is how trust is built.

**`confirm-destructive`** (after initial trust):
Read-only and safe commands run automatically. Commands that modify files,
install packages, or touch system config require confirmation. The agent
classifies commands using pattern matching and a conservative default
(unknown = confirm).

**`auto`** (full trust):
Everything runs automatically except commands matching `alwaysConfirm`
patterns (sudo, rm -rf, etc.). The operator has decided they trust the agent.

The trust level can be changed at any time via a command in the agent UI.
The `alwaysConfirm` list is always respected regardless of trust level.

## Testing Strategy

### Layered Approach

1. **Unit tests**: Pure functions (message formatting, cost calculation,
   tool parsing)
2. **Component tests**: Ink components via ink-testing-library (render to
   string, assert on text output)
3. **Integration tests**: Agent core + mock provider (no real API calls)
4. **E2E tests**: ink-testing-library for full UI scenarios — render the
   app, send input, assert on output. All text-based, all readable by the
   agent.
5. **Self-tests**: Agent drives itself through scenarios (programmatic
   API, not UI)

### Why ink-testing-library Works for Agent Self-Iteration

The agent can:
- Render any component and read the output as a string
- Send keystrokes and input programmatically
- Assert that the right text appears
- Do all of this in-process, fast, no external dependencies

This is vastly simpler than Playwright for the agent's purposes. The agent
reads text natively. The UI outputs text. Perfect match.

### Self-Operation

The agent can also test itself via the **Agent API** directly (bypassing UI):
- Send a message programmatically
- Observe the response, tool calls, token usage
- Verify correctness

This is even faster than UI tests and validates core logic independent of
rendering.

### Test Infrastructure

- Test runner: `bun test` (native, fast, built-in) or Vitest
- Component testing: ink-testing-library
- API mocking: Record/replay of SSE streams, or mock provider adapter
- CI: Tests run on every self-modification before the new version is accepted

## UI Architecture

### Abstract Layout Model

The layout is defined as a **logical structure** independent of rendering
technology. This same structure drives the Ink renderer and (later) the
browser renderer.

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

In Ink, the Static Zone maps directly to `<Static>`. The Live Zone is the
re-rendering area. The Status Bar is a fixed-position bottom row.

See `DESIGN-UI.md` for the full layout descriptor specification.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│              Layout Descriptor (abstract)            │
│  Defines panes, sizes, content bindings              │
├──────────────────┬──────────────────────────────────┤
│  Ink Renderer    │  (Future) Vite + React            │
│  Terminal UI     │  Browser UI                       │
├──────────────────┴──────────────────────────────────┤
│                    Agent Core                        │
│  - Conversation loop                                 │
│  - Tool dispatch (full machine access)               │
│  - Trust policy enforcement                          │
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
│  - Log projection (compact view for self-analysis)   │
├─────────────────────────────────────────────────────┤
│              Test Harness                            │
│  - ink-testing-library (components + E2E)            │
│  - Mock provider adapter (record/replay)             │
│  - Self-operation interface (agent API)              │
│  - E2E scenario runner                               │
└─────────────────────────────────────────────────────┘
```

## Log Projection for Self-Analysis

Raw logs are written to disk in full — every API request body, every response,
every tool output. Nothing is discarded. But when the agent reads its own
logs for self-analysis (debugging, performance review, self-improvement), the
full payloads would flood the context window and defeat the purpose.

The observability layer provides a **projection** mode for log queries:

```
Full log entry (on disk):
{
  "type": "api_request",
  "timestamp": "...",
  "model": "claude-opus-4-6",
  "request": { "system": "You are Omega...(2,847 chars)", "messages": [...] },
  "response": { "content": "Here is the fix...(14,203 chars)", ... },
  "usage": { "input_tokens": 3847, "output_tokens": 1204 }
}

Projected log entry (for self-analysis):
{
  "type": "api_request",
  "timestamp": "...",
  "model": "claude-opus-4-6",
  "request_size": 12403,
  "request_hash": "a7f3c2...",
  "response_size": 14203,
  "response_hash": "e91b4d...",
  "system_prompt_tokens": 428,
  "message_count": 12,
  "tool_calls": ["read_file", "run_command"],
  "usage": { "input_tokens": 3847, "output_tokens": 1204 },
  "cost_usd": 0.042,
  // Provider timing
  "provider_ttft_ms": 340,        // time to first token from provider
  "provider_total_ms": 2100,      // full response time from provider
  // Agent timing
  "agent_prep_ms": 12,            // time agent spent building the request
  "agent_tool_dispatch_ms": 845,  // time executing tool calls in this turn
  "agent_think_ms": 3,            // time between response received and next action
  "turn_total_ms": 2960           // wall clock for the full turn
}
```

The projection replaces large text fields with size + hash, while keeping
all structural and numeric metadata — including full timing breakdown. The
agent can:
- Spot anomalies (why did this call use 50k input tokens?)
- Track cost trends
- Identify which tool calls are expensive
- Find its own bottlenecks (is tool dispatch slow? is request prep slow?)
- Compare provider latency across calls and models
- Drill into a specific entry by hash if it needs the full content

The projection is not a separate log — it's a **query mode**. The agent
asks for logs in projected form by default. If it needs the raw content of
a specific entry, it fetches it by ID or hash.

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
- **Trust policy interface**: How trust levels and confirmation work

This guarantees that even if the agent introduces a bug in surface code, it
retains the ability to:
1. Read its own source
2. Run its tests
3. Identify the failure
4. Fix the code
5. Validate the fix

## Milestones

### M0 — Minimal Viable Agent (bootstrap target)
- [ ] Project setup (TypeScript, Ink, Anthropic SDK, `bun test`)
- [ ] Agent core: send messages to Anthropic API with streaming
- [ ] Terminal UI: display streamed response in Ink
- [ ] Modal input: normal mode / insert mode (minimal, Helix-inspired)
- [ ] Model payload viewer (collapsible, shows what goes to the model)
- [ ] Log every API call with token counts
- [ ] Status bar showing mode + model + token usage
- [ ] First E2E test using ink-testing-library

### M1 — Self-Improvement Loop (stable core target)
- [ ] Tools: read file, write file, run shell command, list files
- [ ] Tool: web search (DuckDuckGo or similar, no API key)
- [ ] Tool: fetch URL (readability-mode, extract text from HTML)
- [ ] Trust policy: confirm-all mode (operator approves every command)
- [ ] Agent can modify its own code
- [ ] Spawn-validate-swap self-modification flow
- [ ] Structured logging (JSON lines)
- [ ] Frozen core interfaces
- [ ] Test suite the agent runs on itself before accepting changes
- [ ] Cost/token tracking with session aggregation

### M2 — Full Machine Agent
- [ ] Trust levels: confirm-all → confirm-destructive → auto
- [ ] Trust policy configurable at runtime (command in UI)
- [ ] `alwaysConfirm` / `alwaysAllow` pattern lists
- [ ] sudo handling: detect need, prompt operator, execute
- [ ] Rich command output display (ANSI, truncation, scrolling)
- [ ] Abstract layout descriptor driving Ink rendering
- [ ] Side pane with observability data (toggleable)
- [ ] Scrollable history for completed turns
- [ ] Conversation history persistence
- [ ] Keyboard shortcuts / command palette

### M3 — Coding Agent Features
- [ ] Project context awareness (file tree, git status)
- [ ] Intelligent file search (grep, glob)
- [ ] Multi-file editing with diff review
- [ ] Test execution and error analysis
- [ ] Agent self-tests its coding features

### M4 — Browser UI (rich human experience)
- [ ] Vite + React renderer consuming the same layout descriptor
- [ ] Browser DevTools for human debugging
- [ ] Parity with terminal UI features
- [ ] Playwright E2E tests for browser

## Future Considerations

These are not planned for any milestone yet but should not be designed out.

### Voice Input

The operator dictates into the agent by voice. A local speech-to-text model
(already prototyped in a separate repository) transcribes and feeds text into
the input. Alternatively, the main model's provider could be used for
transcription if it offers better context-aware accuracy. The input interface
should accept text from any source — keyboard, pipe, or external process —
so voice integration is a matter of wiring, not architecture.

### Helix Keymap Details

The full keymap (selections, motions, text objects, multiple cursors) is
deferred. The relevant architectural constraint is: the input layer must
support modal dispatch (keystrokes routed differently based on current mode)
and the UI must indicate the active mode visibly.

## Next Steps

1. **Set up project** — `bun init`, Ink, Anthropic SDK, `bun test`
2. **Build M0** — minimal streaming chat with token logging in the terminal
