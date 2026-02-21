# Omega — Self-Improving AI Coding Agent

*Configuration is code — edit TypeScript, rebuild, done. No YAML, no TOML,
no JSON config files, no schema validation, no config loading layer.
Minimal where it matters, pragmatic where it doesn't.*

## Current Status

**You are Omega.** Source code in `src/` is your codebase. Modifying it
modifies yourself. Run `bun start` to launch, `bun run login` to auth,
`bun test` to test (155 pass, 4 pre-existing failures).

- **M0–M2 complete.** Streaming, tools, trust, self-modification loop,
  logging, truncation, frozen interfaces. See milestones below.
- **M3 in progress.** Session persistence done. UI tests and observability
  remaining. See M3 checklist in milestones.
- **Model**: `claude-sonnet-4-6` (change in `config.ts`)
- **Auth**: Claude Max via OAuth through `claude.ai` + identity headers.
  See `docs/oauth-pitfall.md`. Falls back to `ANTHROPIC_API_KEY` env var.
- **Auto-approve**: read-only tools, file writes, safe shell commands
  (including compound `&&`/`;` chains). Config in `config.ts`.
- **Esc interrupts streaming**, stream ordering errors auto-retry.
- **Known issue**: dictation truncation with `wtype` (Wayland).

### Project Structure

```
omega/
  plan/              ← you are here (planning docs, source of truth)
    overview.md      ← this file
    ui.md            ← UI layout and interaction design
  docs/
    oauth-pitfall.md ← OAuth flow details (exact URLs, params, headers)
    lessons-learned.md ← methodology lessons (read before new features)
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

A personal, self-improving AI coding agent. Terminal-first (Ink), single-user,
full machine access. Can bootstrap itself: edit source → test → commit/revert → restart.

## Design Principles

1. **Tight iteration loop** — edit, test, reload with minimal friction
2. **Terminal-native** — Ink UI; agent reads its own text output
3. **Observability first** — structured logs, traces, token/cost tracking
4. **Full payload visibility** — operator sees everything sent to the model
5. **Streaming** — first-token latency; never block the UI
6. **Small-window-first** — works in narrow terminals; panes collapsed by default
7. **Self-testable** — agent can drive itself for E2E tests
8. **Stable core, evolvable surface** — frozen interfaces from M2
9. **Full machine access** — only `sudo` requires operator intervention
10. **Graduated trust** — confirm-all → confirm-destructive → auto
11. **Session handoff** — plan/ docs as handoff between sessions/models
12. **Helix-style modal input** (future) — normal + insert mode

## Key Decisions

- **TypeScript + Bun** — native TS, fastest startup for self-restart loop
- **Ink over browser** — agent's native medium is text; browser deferred
- **Config is code** — edit `config.ts`, rebuild, done. No YAML/JSON.
- **Truncation over summarization** — drop oldest messages, keep system + recent
- **Planning files as persistent context** — survive truncation via re-read
- **Red-green mandatory** — test must fail first; enforced in system prompt
- **Three error categories**: (A) our bugs → loud errors; (B) provider → retry
  with backoff; (C) operational limits → detect and report

## Architecture

```
Ink Renderer → Agent Core → Provider Adapter → Observability Layer
```

Agent core never imports react/ink. Tools have full machine access.
Self-modification: uncommitted = experiment, committed = stable.
Frozen interfaces: tool, provider adapter, observability, trust policy.

See `docs/oauth-pitfall.md` for auth details, `docs/lessons-learned.md`
for methodology rules.

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
- [x] **API call visibility** (v1 — UX rework needed, see below)
      - `api_call_start` event emitted by `Agent` before each stream call.
      - Turn separators and payload panel shipped but have UX issues.
- [x] **API call visibility UX rework**
      - Panel never auto-opens; `i` toggles (mnemonic: inspect), `q` closes.
      - Removed `p` keybinding and panel from Esc chain entirely.
      - Panel pinned just above input box (inside live zone) — not pushed
        off-screen by new static output.
      - Panel capped at 20 messages; shows call number and truncation notice.
      - Turn separator: bold cyan `▶ Turn N ~X tokens` (was: weak dim line).
- [x] **Input prompt blocked-state visual**
      - Bug: while agent is acting, prompt shows normal green `❯` — looks
        identical to idle/ready state. Input is actually unfocused/blocked
        but there is no visual cue.
      - Fix: while streaming (and not pending tool confirmation), show a
        dim `… ` glyph instead of the green `❯`, and dim the placeholder
        text. The `❯` only appears when the user can actually type.
- [x] **API call inspector UX fixes**
      - Bug 1: `i`/`q` shortcuts fired while user was typing in the prompt
        (useInput fires for all keypresses regardless of TextInput focus).
        Fix: extracted `shouldHandleShortcut()` pure function to `ui-logic.ts`
        (10 unit tests); gates on `inputState.length === 0`.
      - Bug 2: separator said "Turn N" → renamed to "API call #N".
      - Bug 3: panel always shows most recent API call; call number in title
        makes that clear. No navigation needed for now.
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
- [x] **Web search tool**
      - DuckDuckGo Instant Answer API (no API key) → JSON; falls back to
        HTML scrape if instant answer is empty
      - `web_search(query)` → abstract, answer, top 8 results with URLs + snippets
      - Auto-approved; 4 tests in `tools.test.ts`
- [x] **URL fetcher**
      - `fetch_url(url)` → HTML stripped to plain text, truncated at 8000 chars
      - URL validated (must be http/https); graceful errors on bad host/protocol
      - Auto-approved; 5 tests in `tools.test.ts`
      - Note: uses `rejectUnauthorized: false` (Bun TLS doesn't trust system CA on this machine)
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

1. **Automated plan maintenance** ← NEXT
   The operator has to manually ask me to update the plan after each change,
   and the Next Steps section keeps getting stale/duplicated. Options to
   explore:
   - Agent reads plan/overview.md at the start of every session and proposes
     a diff to Next Steps before doing any work.
   - A post-commit hook (or self.ts step) that prompts the agent to update
     the plan after every commit.
   - Keep Next Steps to ≤5 items, always in priority order, always reflecting
     actual current state. Agent is responsible for keeping it clean.
   Decision needed: pick an approach and implement it.

2. **UI tests** — ink-testing-library coverage for `ui.tsx`.

3. **Trust levels** — confirm-destructive mode so auto-approve is broader.

4. **Dictation truncation bug** — `wtype` injects keystrokes one at a time
   via Wayland; truncation still occurs despite the `useEffect` fix.
