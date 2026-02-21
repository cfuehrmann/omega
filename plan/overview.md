# Omega — Self-Improving AI Coding Agent

*Configuration is code — edit TypeScript, rebuild, done. No YAML, no TOML,
no JSON config files, no schema validation, no config loading layer.
Minimal where it matters, pragmatic where it doesn't.*

## Current Status

**You are Omega.** You are reading your own planning documents. The source
code in `src/` is your own codebase. When you modify files in this project,
you are modifying yourself.

- **M0 and M1 are complete.** You can stream responses, use tools (read/write
  files, run commands, list directories), with a graduated trust policy.
- **M2 is complete.** Self-modification loop: edit source, run `bun test`,
  git commit on pass, git revert on fail, restart. Structured JSON-lines
  logging (`src/logger.ts`), context window truncation, frozen core interfaces.
  81 tests, all passing.
- **Auto-approve is implemented.** Read-only tools (`read_file`, `list_files`)
  and `write_file` are auto-approved without operator confirmation. Safe shell
  commands (`ls`, `cat`, `grep`, `git status/log/diff`, `bun test`, and all
  self-modification git commands) are auto-approved. Compound commands
  (`cmd1 && cmd2` or `cmd1; cmd2`) are approved only when every part is
  individually approved; a `cd` into a relative project subdirectory (no
  absolute path, no `..`) also counts as approved. The auto-approve logic
  lives in `agent.ts` and skips the `tool_pending` event entirely (no UI
  latency). Config lists are in `config.ts` (`autoApproveTools`,
  `autoApproveCommands`). Truly destructive commands (e.g. `rm -rf`) still
  require operator confirmation.
- **Claude Max authentication is implemented and verified.** Run
  `bun run login` to authenticate. The OAuth token alone is NOT enough —
  it authenticates but bills per-token. The login flow exchanges the OAuth
  token for an API key via `/api/oauth/claude_cli/create_api_key` (same as
  Claude Code does). That API key carries Claude Max billing. On every
  startup, the key is **verified** against the API (free `count_tokens`
  call + rate limit header check) to confirm billing type. The first line
  in the UI shows the verified result. See `docs/oauth-pitfall.md` for the
  full explanation. Falls back to `ANTHROPIC_API_KEY` env var if no OAuth.
  Token in `~/.config/omega/oauth-token.json`, API key in
  `~/.config/omega/api-key`.
- **Model is currently `claude-sonnet-4-6`** (switched from Opus for cost
  savings). Change in `src/config.ts`.
- **M3 is in progress.** Conversation history persistence is done:
  `src/session.ts` saves history to `~/.local/share/omega/sessions/` after
  every turn; `ui.tsx` offers a resume prompt on startup. The `Agent` class
  accepts an injectable `StreamProvider` for testing. 148 tests passing.
- **Test coverage audit completed.** The test suite is layered correctly:
  unit tests pin pure functions with boundary cases; integration tests
  (mock provider) verify the full `sendMessage` loop, tool dispatch, session
  persist/resume wiring; stream tests anchor the stuck-UI regression.
  **One gap remains: UI behaviour (`ui.tsx`) is not tested.** The resume
  prompt, tool confirmation flow, and streaming display logic are exercised
  only by manual testing. UI tests via `ink-testing-library` are the next
  testing priority.
- **Self-modification loop is fully unattended.** `git add`, `git commit`,
  `git reset`, `git checkout`, `git clean`, and `git rev-parse` are all
  auto-approved. The entire test→commit/revert cycle runs without operator
  confirmation.
- **Stream ordering errors are now retried.** The Anthropic SDK throws
  `"Unexpected event order, got message_start before receiving message_stop"`
  when the server restarts a stream mid-flight. `isRetryable()` now matches
  this error by message text and retries automatically (up to 5 times).
- Run `bun start` from the project root to launch yourself.
- Run `bun run login` to authenticate with Claude Max.
- **Compound command auto-approve.** `cd <subdir> && grep ...` and similar
  compound commands are now split on `&&`/`;` and each part checked
  independently. `cd` into a relative project path is safe; any part that
  isn't approved blocks the whole command.
- **Escape interrupts streaming.** Pressing Esc mid-response aborts the
  current stream via `AbortController`. The partial text is flushed to
  history, no incomplete assistant turn is added, and the operator can
  immediately type a correction. The status bar shows `│ Esc to interrupt`
  only while a stream is interruptible (not during tool confirmation).
- **Dictation truncation partially fixed.** `fast-text-input.tsx` `useEffect`
  now only resets `valueRef` when `value === ""` (external clear after submit),
  never on intermediate prop echoes. Root cause of remaining truncation is
  still under investigation — `wtype` (Wayland key injector) sends keystrokes
  individually; the exact truncation mechanism needs more diagnosis.
- Run `bun test` to run the test suite (132 tests, 7 files).

### Project Structure

```
omega/
  plan/              ← you are here (planning docs, source of truth)
    overview.md      ← this file
    ui.md            ← UI layout and interaction design
  src/
    agent.ts         ← agent core (streaming, tool loop, retry, auto-approve, truncation)
                        StreamProvider interface for test injection
    auth.ts          ← OAuth PKCE flow for Claude Max
    config.ts        ← model, system prompt, settings (TypeScript, not YAML)
    fast-text-input.tsx ← custom text input (fixes paste/dictation issues)
    logger.ts        ← structured JSON-lines logger (→ ~/.local/share/omega/logs/)
    login.ts         ← interactive login script (bun run login)
    self.ts          ← self-modification orchestration (test→commit/revert→restart)
    session.ts       ← session persistence (save/load/list, ~/.local/share/omega/sessions/)
    tools.ts         ← tool definitions and execution (read/write/run/list/edit_file)
    ui.tsx           ← Ink terminal UI (static zone, live zone, status bar, resume prompt)
    main.tsx         ← entry point
    *.test.ts        ← 132 tests across 7 files
  package.json
```

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
6. **Small-window-first** — The UI must work well in a small terminal window.
   Panes are collapsible (and collapsed by default where appropriate). The
   layout degrades gracefully. Full-screen is a luxury, not a requirement.
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
- Built-in package manager (no separate pnpm/npm needed)
- Full compatibility with the Anthropic SDK
- Ink 6.x + ink-testing-library 4.x confirmed working (smoke-tested)
- npm-compatible — full ecosystem access

**Verified compatibility** (2025-02-21):
- Ink rendering, flexbox layout, borders, text styling ✅
- `<Static>` zone ✅
- State updates / streaming simulation ✅
- `ink-testing-library` render + assert ✅
- `useInput` with TTY ✅
- Clean exit ✅

### Model: Claude claude-opus-4-6 (starting point)

The operator has a Claude Max account. We start with `claude-opus-4-6` as
the fixed model. Provider abstraction (OpenAI, local LLMs, etc.) is a future
concern — the provider adapter interface will make it possible, but we don't
build for it now.

### UI Strategy: Terminal-First (Ink), Browser Later

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
- Vite + React will be added as a second renderer much later.
- At that point, extract an abstract layout descriptor from the Ink
  implementation. Not before — we'd be guessing at the interface.

**Architecture implication:**
```
Agent Core (pure TypeScript — no framework imports)
    ↓ exposes
Agent API (function calls, events, state)
    ↓ consumed by
Terminal Renderer (Ink)             ← now
(Browser Renderer)                  ← much later
```

The agent core must never import from `react` or `ink`. The renderer is a
thin consumer of the core API.

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

### Configuration

Secrets (API keys) are stored outside the repo — environment variables or
a `.env` file in `.gitignore`. Everything else is TypeScript:

```typescript
// config.ts — checked into git
export const config = {
  model: 'claude-opus-4-6',
  maxContextTokens: 100_000,     // truncation target
  trustLevel: 'confirm-all' as const,
  alwaysConfirm: ['rm -rf', 'sudo', 'reboot', 'dd ', 'mkfs'],
  alwaysAllow: ['ls', 'cat', 'git status', 'git log', 'grep', 'find', 'wc'],
}
```

No config format to learn or parse. Change the source file, restart.

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
specific URLs with a readability-mode fetcher that extracts article text
from HTML.

This is a first-class tool, not an afterthought. An agent that can't search
is guessing when it could be looking things up. Added in M4.

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

## Context Window Management

Claude claude-opus-4-6 has a 200k token context window. Every API call sends the
*entire* conversation history — there's no server-side memory.

The problem: after 20–30 tool calls, context can easily reach 50–100k tokens.
Each call pays for *all* accumulated history again. Cost grows quadratically
with conversation length.

### Strategy: Truncation with Token Budget

Start simple. Set a budget (e.g., 100k tokens). When the conversation exceeds
the budget:

1. **Always keep**: system prompt, tool definitions, the original task message
2. **Always keep**: the last N turns (e.g., last 10)
3. **Drop**: oldest middle messages first
4. **Future enhancement**: replace dropped messages with a one-paragraph
   summary (costs one extra API call but preserves key context)

The token count is tracked per-message (the provider adapter records it).
When building a request, the agent sums from newest to oldest and stops
adding messages when the budget is reached.

The payload viewer (Principle 11) makes this visible: the operator sees
exactly which messages are included and which were truncated.

### Planning Files as Persistent Context

Truncation is low-risk for this project because the ground truth lives in
files, not in conversation history. The planning files in `plan/` and the
source code are always available via tool calls.

The system prompt makes this explicit:

> Your project's planning files are in `plan/`. They are the source of truth
> for goals, architecture, and decisions. If your conversation history has
> been truncated and you've lost context, `ls plan/` and re-read the files.
> They are always current.

This means the agent can recover from aggressive truncation: it loses the
conversational flow but can reconstruct what it was doing by reading the
plan. This is also why we keep the planning files up to date — they serve
double duty as documentation and as recoverable context.

### What This Means in Practice

- Short tasks (< 20 turns): no truncation, full history
- Medium tasks (20–50 turns): oldest turns get dropped, recent work preserved
- Long tasks: the operator may need to start a new session with a handoff
  summary (Principle 12)
- In all cases: planning files are re-readable, so context loss is recoverable

Summarization is a future enhancement. Truncation is the M2 implementation.

## Error Handling

### Three Categories

**A. Our bugs** — Malformed request, crash during tool execution, can't parse
a valid response. These are code errors.

Strategy: surface loudly with full stack trace. Log everything. The agent (or
operator) fixes the code. No retry — retrying a bug doesn't help.

**B. Provider/infrastructure** — Rate limits (HTTP 429), server overload
(529), network timeouts, API down. These are transient.

Strategy: retry with exponential backoff. Start at 1s, double each retry, cap
at 60s, max 5 attempts. Show the operator what's happening ("Rate limited,
retrying in 8s..."). Log every retry. If all retries fail, surface the error
and let the operator decide.

The status bar shows provider status: 🟢 when healthy, 🟡 when retrying,
🔴 when failed.

**C. Operational limits** — Model refuses a request, output truncated
(hit `max_tokens`), unexpected `stop_reason`, content filter triggered.

Strategy: detect and report clearly. These aren't failures to retry — they're
signals that need a different approach:
- Truncated output → continue the response ("please continue")
- Refusal → report to operator, let them rephrase
- Unexpected stop_reason → log and surface

## Testing Strategy

### Red-Green Testing (mandatory)

Every bug fix and every feature MUST follow red-green discipline:

1. **Red**: Write a test that describes the desired behavior. Run it.
   It MUST fail. If it passes immediately, the test is wrong — it's not
   testing what you think. Rewrite until it fails.
2. **Green**: Change production code to make the failing test pass.
   Run all tests. They must all pass.
3. **Commit**: Only commit when all tests are green.

**Why this matters for a self-improving agent:** When the agent writes both
the test and the fix together, the test might accidentally pass for the
wrong reason (e.g., testing the new code path instead of the broken one,
or not exercising the actual edge case). A test that never failed has never
proven it catches anything. Red-green eliminates this class of false
confidence.

**The system prompt enforces this.** The agent is instructed to follow
red-green in `config.ts`. If you see the agent skip the red step, that's
a bug in the agent's behavior.

### Layered Approach

1. **Unit tests**: Pure functions (message formatting, cost calculation,
   tool parsing, UI event→state mapping)
2. **Component tests**: Ink components via ink-testing-library (render to
   string, assert on text output)
3. **Integration tests**: Agent core + mock provider (no real API calls)
4. **E2E tests**: ink-testing-library for full UI scenarios — render the
   app, send input, assert on output. All text-based, all readable by the
   agent.
5. **Self-tests**: Agent drives itself through scenarios (programmatic
   API, not UI)

### Test Infrastructure

- Test runner: `bun test` (native, fast, built-in)
- Component testing: ink-testing-library
- API mocking: Record/replay of SSE streams, or mock provider adapter
- CI: Tests run on every self-modification before the new version is accepted

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Ink Renderer (Terminal UI)                          │
│  - React components consuming agent state            │
│  - Modal input dispatcher (normal / insert)          │
│  - Collapsible panes, status bar                     │
├──────────────────────────────────────────────────────┤
│                    Agent Core                        │
│  - Conversation loop                                 │
│  - Context window management (truncation)            │
│  - Tool dispatch (full machine access)               │
│  - Trust policy enforcement                          │
│  - Self-modification orchestration                   │
├──────────────────────────────────────────────────────┤
│               Provider Adapter                       │
│  - Anthropic Messages API (streaming SSE)            │
│  - Request/response logging                          │
│  - Token counting & cost estimation                  │
│  - Cache control management                          │
│  - Retry with backoff (429, 529, network errors)     │
├──────────────────────────────────────────────────────┤
│              Observability Layer                     │
│  - Structured log sink (file + in-memory ring)       │
│  - API call trace store                              │
│  - Metrics aggregation                               │
│  - Self-analysis hooks (agent can query its logs)    │
│  - Log projection (compact view for self-analysis)   │
└──────────────────────────────────────────────────────┘
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

The agent modifies its own source code and validates the changes using git
as the safety net.

### Workflow (simple, initial)

1. **Edit** — Agent writes changes to source files (uncommitted)
2. **Test** — Agent runs `bun test` against the modified code
3. **Commit or revert**:
   - Tests pass → `git add -A && git commit -m "..."`
   - Tests fail → `git checkout .` (revert all changes)
4. **Restart** — Agent restarts itself with the new (or reverted) code

The working tree is the experiment. Committed code is stable. This is simple
and sufficient for early self-improvement.

### Future Enhancement: Two-Instance Comparison

Later, the agent could run two instances side-by-side — the stable committed
version and the experimental modified version — and compare behavior before
committing. This adds complexity (port conflicts, shared state) and is
deferred until the simple restart flow feels limiting.

### Git Hygiene

- Every self-modification is a commit with a descriptive message
- The git log is an audit trail of the agent's evolution
- State (conversation history, config) is persisted to disk so it survives
  restarts
- `.gitignore` excludes logs, secrets, and ephemeral state

## Stable Core Contract

From M2 onward, the following interfaces are frozen (backwards-compatible
changes only):

- **Tool interface**: How tools are defined, dispatched, and return results
- **Provider adapter interface**: How the agent talks to the LLM
- **Observability hooks**: How logs and metrics are emitted
- **Trust policy interface**: How trust levels and confirmation work

This guarantees that even if the agent introduces a bug in surface code, it
retains the ability to:
1. Read its own source
2. Run its tests
3. Identify the failure
4. Fix the code
5. Validate the fix

## Milestones

### M0 — Talking to the Model ✅
- [x] Project setup: `bun init`, Ink, Anthropic SDK
- [x] Agent core: send a message to Claude claude-opus-4-6, stream the response
- [x] Terminal UI: display the streamed response in Ink
- [x] Show token count and cost after each response
- [x] Basic input: type a message, send it, see the response
- [x] Conversation history (in-memory, multi-turn works)

### M1 — Basic Tools + Trust ✅
- [x] Tools: read file, write file, run shell command, list files
- [x] Trust policy: confirm-all mode (operator approves every command)
- [x] Tool results displayed in the UI
- [x] Agent can use tools in a loop (model calls tool → gets result → responds)
- [x] Basic error handling (retry with backoff for provider errors)
- [x] Auto-approve for read-only tools and safe commands (post-M1 addition)

### M2 — Self-Improvement Loop ✅
- [x] Agent can modify its own source files
- [x] Agent runs `bun test` to validate changes
- [x] Git commit on success, git revert on failure
- [x] Agent restarts itself after successful self-modification
- [x] Structured logging (JSON lines to disk → `~/.local/share/omega/logs/`)
- [x] Frozen core interfaces
- [x] Context window management (truncation with token budget)
- [x] Cost/token tracking with session aggregation
- [x] `edit_file` tool for surgical edits (red-green tested)
- [x] Fixed input: custom fast-text-input (paste/dictation reliability)
- [x] Red-green testing enforced in system prompt and plan

After M2, the agent can improve itself. This is the stable core target.

### M3 — Observability + Rich UI
- [x] **Conversation history persistence**
      - `src/session.ts`: `saveSession` / `loadLatestSession` / `listSessions`
      - Saves to `~/.local/share/omega/sessions/<sessionId>.json` after every turn
      - `Agent`: `sessionId`, `checkPriorSession()`, `resumeSession()`, `persistSession()`
      - `Agent` constructor accepts `StreamProvider` + `sessionDir` for test injection
      - `ui.tsx`: cyan resume prompt on startup (Y=restore history, N=fresh start)
      - 14 unit tests (`session.test.ts`) + 20 integration tests (`agent-integration.test.ts`)
- [x] **Integration test layer** (added alongside persistence)
      - `StreamProvider` interface: injectable mock replaces real Anthropic client
      - Covers: text response, tool loop, tool rejection, history growth, session
        persist/resume, error handling, retry-then-succeed
      - Test isolation: per-test temp dirs (avoid fire-and-forget persist races)
      - Discovered and documented: `params.messages` is a live reference to
        `this.history` — tests must snapshot with `[...params.messages]`
- [x] **Self-modification loop fully unattended**
      - `git add`, `git commit`, `git reset`, `git checkout`, `git clean`,
        `git rev-parse` added to `autoApproveCommands` in `config.ts`
      - Entire test→commit/revert cycle runs without operator confirmation
- [x] **Stream ordering errors retried automatically**
      - `isRetryable()` extended to match SDK-level `"Unexpected event order"`
        errors by message text (no HTTP status code on these)
      - Agent retries up to 5× instead of surfacing a hard error
- [x] **Escape interrupts streaming**
      - `sendMessage()` accepts optional `AbortSignal`; stream loop checks
        `signal.aborted` after each event and breaks cleanly
      - Emits `{ type: "interrupted" }` event; partial text flushed, no
        incomplete assistant turn added to history
      - `ui.tsx`: Esc fires `abortControllerRef.current.abort()`
      - Status bar shows `│ Esc to interrupt` only while interruptible
        (streaming, no pending tool confirmation)
      - Input box always visible; unfocused during streaming so Esc reaches
        the global `useInput` handler
- [~] **Dictation truncation bug** — `useEffect` fix landed (only resets ref
      when `value === ""`). Root cause confirmed as `wtype` injecting keys
      one at a time via Wayland. Further diagnosis needed to understand
      exactly where truncation occurs and whether additional buffering is
      required in `fast-text-input.tsx`.
- [ ] **API call visibility** ← NEXT
      - Two-part feature: turn separators + payload inspector panel.
      - Add `api_call_start` event to `AgentEvent` carrying a snapshot of
        the full `streamParams` (model, system prompt, tool definitions,
        messages, estimated token count). Emitted just before each stream
        call inside the agentic loop.
      - **Turn separators**: handle `api_call_start` in `ui.tsx` — push a
        dim `── API call #N ─── ~X tokens ───` line into the static zone so
        the boundary between every round-trip is visible in the scrollback.
      - **Payload panel**: press `p` (while not in the input box) to toggle
        a `PayloadPanel` component in the live zone. Shows the last API
        call's full context — model, tokens, cost, system prompt size, tool
        count, and each conversation message summarised. Sub-sections
        expandable individually. `p` again or `Esc` closes it.
      - Payload formatting logic is pure/unit-tested. Panel is a new Ink
        component in `ui.tsx`.
- [ ] **UI tests** — `ui.tsx` has zero automated tests. Use
      `ink-testing-library` to cover: resume prompt, tool confirmation,
      streaming display, Esc interrupt, payload panel toggle.
- [ ] Log projection for self-analysis
- [ ] Modal input: normal mode / insert mode (Helix-inspired)
- [ ] Side pane with observability data (collapsible, collapsed by default)
- [ ] Status bar: mode, model, tokens, cost, latency, provider health
- [ ] Scrollable history for completed turns
- [ ] Keyboard shortcuts

### M4 — Full Machine Agent
- [ ] Trust levels: confirm-all → confirm-destructive → auto
- [ ] Trust policy configurable at runtime
- [ ] `alwaysConfirm` / `alwaysAllow` pattern lists
- [ ] sudo handling: detect need, prompt operator, execute
- [ ] Web search tool (DuckDuckGo, no API key)
- [ ] URL fetcher (readability-mode, extract text from HTML)
- [ ] Rich command output display (ANSI, truncation, scrolling)

### M5 — Coding Agent Features
- [ ] Project context awareness (file tree, git status)
- [ ] Intelligent file search (grep, glob)
- [ ] Multi-file editing with diff review
- [ ] Test execution and error analysis
- [ ] Agent self-tests its coding features
- [ ] E2E tests using ink-testing-library

### Future
- [ ] Browser UI (Vite + React, abstract layout extracted from Ink impl)
- [ ] Provider abstraction (OpenAI, local LLMs)
- [ ] Voice input (local STT or provider transcription)
- [ ] Helix keymap details (selections, motions, text objects)
- [ ] Context summarization (replace truncated messages with summaries)
- [ ] Two-instance self-modification comparison

## Next Steps

1. **API call visibility** ← NEXT — turn separators + `p`-key payload panel.
   See M3 checklist above for full spec.

2. **UI tests** — ink-testing-library coverage for `ui.tsx`.

3. **Dictation truncation bug** — `wtype` injects keystrokes one at a time
   via Wayland; truncation still occurs despite the `useEffect` fix. Needs
   debug logging to pinpoint the drop site.

4. **Remaining M3 items** — log projection, modal input, observability pane,
   scrollable history, keyboard shortcuts.
