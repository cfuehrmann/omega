## Omega — State of the World

### Purpose
Omega is a self-improving coding agent running in a terminal. It edits its own source code in `src/`, runs `bun test`, commits on green, reverts on red, and restarts itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O (`src/ui-raw.ts`). No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/gpt` command and rate-limit fallback.

### Context Management (three-zone model)
- **Zone 1** — `plan/world-state.md` (inside the project repo): LLM-compacted summary of all prior sessions. Loaded at session start into system prompt as `## World State (from previous sessions)`. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown (SIGINT/SIGTERM/Ctrl+C). Lives under source control — travels with the repo, continuity across machines.
- **Zone 2** — turn summaries: after each `turn_end`, completed turn messages are LLM-compacted into a 2-message synthetic exchange (`[session summary…]` / `Understood.`). History is always exactly 2 messages after compaction. Implemented in `src/compaction.ts` via `compactTurn()`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages (safety net).
- Token budget: 100k.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Crash mid-session loses conversational context but not work product (files). Periodic in-session folding deferred.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction functions
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → returns `<cwd>/plan/world-state.md`; `defaultWorldStatePath()` (deprecated alias)
- `src/ui-raw.ts` — raw terminal UI, event rendering, `shutdown()` (folds world state then exits), SIGINT/SIGTERM handlers
- `src/session.ts` — session persistence module (no longer imported by production code; kept for independent tests)
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`
- `src/openai.ts` — OpenAI Codex integration, request building, response parsing
- `src/config.ts` — model (`claude-sonnet-4-6`), fallback model, system prompt, token limits
- `plan/past.md`, `plan/future.md`, `plan/present.md` — planning system; read at session start, update at session end
- `plan/world-state.md` — this file; Zone 1 world state, under source control

### UI
API-terminology display: user message (green), api call (cyan), api response (blue), tool execution (yellow), tool result message (magenta), assistant message (white). Time column left. Everything scrollback — no live zone. `turn_end` line shows `[provider/model] in: … out: … cost: … ttft: …`. Status line shows current provider/model + session totals.

### Provider Display
`turn_end` event includes `provider` and `model` fields. Status line shows current provider/model. `/gpt` switches to OpenAI, `/opus` or `/anthropic` switches back.

### Testing Discipline
Red-green mandatory for bugs/features. Structural invariant tests for refactors. 174 tests across ~15 files, all passing. Compaction calls use the same injectable `StreamProvider` as real turns — tests use mock providers.

### Open Issues (from `plan/future.md`)
1. Token efficiency + OpenAI-first provider design (top priority)
2. Provider-specific rate-limit retry policy
3. UI tests for `ui-raw.ts`
4. `sudo` handling
5. Rich command output (truncation, scrolling)
6. Full-screen TUI or browser UI
7. Provider abstraction (clean interface, per-provider settings)

### What Was Accomplished in the Last Session
- Moved world-state file into the repo: `plan/world-state.md` (previously `~/.local/share/omega/world-<slug>-<hash6>.md`).
- `projectWorldStatePath(cwd)` now returns `<cwd>/plan/world-state.md` — simple, no hash, no slug, under source control.
- Removed unused `homedir` and `createHash` imports from `src/world-state.ts`.
- Updated `fold-at-quit.test.ts` to assert the new path format.
- 174 tests passing.
