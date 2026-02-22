## Omega — State of the World

### Purpose
Omega is a self-improving coding agent running in a terminal. It edits its own source code in `src/`, runs `bun test`, commits on green, reverts on red, and restarts itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O (`src/ui-raw.ts`). No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/gpt` command and rate-limit fallback.

### Context Management (three-zone model)
- **Zone 1** — `plan/world-state.md` (inside the project repo): LLM-compacted summary of all prior sessions. Loaded at session start into system prompt as `## World State (from previous sessions)`. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown (SIGINT/SIGTERM/Ctrl+C). Lives under source control.
- **Zone 2** — turn summaries: after each `turn_end`, completed turn messages are LLM-compacted into a 2-message synthetic exchange. History is always exactly 2 messages after compaction. Implemented in `src/compaction.ts` via `compactTurn()`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Crash mid-session loses conversational context but not work product (files).

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained by LLM compaction; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.
- `plan/past.md` and `plan/present.md` — **deleted** (redundant/stale).

The system prompt references only `world-state.md` and `future.md`.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; world-state prompt caps last-session section to 1–4 sentences, bans commit hashes and procedural detail
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — raw terminal UI, event rendering, `shutdown()` (folds world state then exits), SIGINT/SIGTERM handlers; `renderStatus(streaming)` returns shortcuts only
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }` — two ANSI-dimmed labelled lines with `turn:` / `session:` prefixes, column-aligned `in:`/`out:`, model and `ttft` on turn line only
- `src/session.ts` — session persistence module (no longer imported by production code; kept for independent tests)
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`
- `src/openai.ts` — OpenAI Codex integration, request building, response parsing
- `src/config.ts` — model (`claude-sonnet-4-6`), fallback model (`gpt-5.2-codex`), system prompt, token limits
- `src/planning-files.test.ts` — structural invariant tests: asserts `future.md` exists, `past.md`/`present.md` do not exist, system prompt references `world-state.md` + `future.md` but not deleted files
- `src/turn-footer.test.ts` — 11 tests for `formatTurnFooter`
- `plan/future.md` — 4 discrete actionable backlog items (see Open Issues below)

### UI — Turn Footer
After each turn, two dimmed lines are printed:
- `turn:   [model] [ttft]  in: X  out: Y  cost: $Z`
- `session:               in: X  out: Y  cost: $Z`

`in:` is column-aligned between both lines. Cost shown in USD. `renderStatus(streaming)` shows only keyboard shortcuts (not repeated token counts); displayed at startup and during streaming.

### Provider Display
`turn_end` event includes `provider` and `model` fields. `/gpt` switches to OpenAI (`gpt-5.2-codex`), `/opus` or `/anthropic` switches back to Anthropic.

### Pricing Table (in `src/agent.ts`)
- `claude-opus-4-6`: $5 input / $25 output per MTok
- `claude-sonnet-4-6`: $3 input / $15 output per MTok
- `claude-sonnet-4-20250514`: $3 input / $15 output per MTok
- `gpt-5.2-codex`: **$0 / $0 (placeholder — needs update to $1.25 / $10.00)**
- Fallback default: $5 input / $25 output per MTok

Note: Dollar costs are meaningless under Claude Max (OAuth/flat-rate). Token counts are accurate.

### Testing Discipline
Red-green mandatory for bugs/features. Structural invariant tests for refactors. 192 tests across ~17 files, all passing. Compaction calls use the same injectable `StreamProvider` as real turns — tests use mock providers.

### What Was Accomplished in the Last Session
Two pricing fixes were made to `src/agent.ts`: Opus 4.6 corrected from $15/$75 to $5/$25 per MTok (committed `7507950`). Investigation confirmed `gpt-5.2-codex` has a placeholder price of $0/$0; correct OpenAI-direct price is $1.25/$10.00 per MTok — fix confirmed by user but not yet applied. The turn footer was also redesigned this session: `src/turn-footer.ts` was created with `formatTurnFooter`, `src/ui-raw.ts` was updated to use it, and `renderStatus` was simplified to shortcuts-only (committed `e51a531`).

### Open Issues (from `plan/future.md`)
1. Provider-specific rate-limit retry policy
2. UI tests for `ui-raw.ts`
3. `sudo` handling
4. Rich command output (truncation, scrolling)

### Pending (not yet committed)
- Update `gpt-5.2-codex` pricing in `src/agent.ts` from `{ input: 0, output: 0 }` to `{ input: 1.25, output: 10 }` — user confirmed, implement next session.