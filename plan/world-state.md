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
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; world-state prompt caps last-session section to 1–4 sentences, bans commit hashes and procedural detail
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — raw terminal UI; `shutdown()` drains the `foldCurrentSessionIntoWorldState()` generator and renders each event; `parseKeys(chunk, callbacks, buf?, options?)` pure function with `options.pasteState` for bracketed paste injection; `setupRawInput` enables bracketed paste mode (`\x1b[?2004h`) on startup; shutdown disables it (`\x1b[?2004l`) before exit; on paste end (`[201~`), echoes full buffer to stdout; `KeyCallbacks` interface; `runApp()` guarded by `if (import.meta.main)`
- `src/ui-raw.test.ts` — 13 tests for `parseKeys` covering Ctrl+C, Enter, Escape, CSI skip, printable accumulation, bracketed paste (no submit on inner newline, full paste submitted on Enter, normal Enter unaffected, stdout echo on paste end)
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }` — two ANSI-dimmed labelled lines with `turn:` / `session:` prefixes, column-aligned `in:`/`out:`, model and `ttft` on turn line only
- `src/session.ts` — session persistence module (no longer imported by production code; kept for independent tests)
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi(prompt, model, provider, options, signal?)` accepts and forwards `AbortSignal` to `fetch`
- `src/config.ts` — model (`claude-sonnet-4-6`), fallback model (`gpt-5.2-codex`), system prompt, token limits
- `src/planning-files.test.ts` — structural invariant tests: asserts `future.md` exists, `past.md`/`present.md` do not exist, system prompt references `world-state.md` + `future.md` but not deleted files
- `src/turn-footer.test.ts` — 11 tests for `formatTurnFooter`
- `src/openai.test.ts` — tests for `buildOpenAiRequest`, `parseOpenAiResponse`, and abort signal forwarding
- `src/fold-events.test.ts` — 8 tests covering generator shape, no events for null path/empty history, `api_call_start`, `api_response` with token usage, `tool_result` with name `write_file`, file written to disk, error event on LLM failure
- `src/fold-at-quit.test.ts` — tests for `foldCurrentSessionIntoWorldState()` as a generator (drains with `for await`)
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
- `gpt-5.2-codex`: $1.25 input / $10.00 output per MTok
- Fallback default: $5 input / $25 output per MTok

Note: Dollar costs are meaningless under Claude Max (OAuth/flat-rate). Token counts are accurate.

### Testing Discipline
Red-green mandatory for bugs/features. Structural invariant tests for refactors. 214 tests across 19 files, all passing. Compaction calls use the same injectable `StreamProvider` as real turns — tests use mock providers.

### What Was Accomplished in the Last Session
Fixed a bug where pasted text was invisibly captured but never echoed to the terminal: `parseKeys` now calls `process.stdout.write(buf.value)` when the bracketed paste end marker `[201~` is received, making pasted content visible immediately.

### Open Issues / Known Problems
1. **Backspace past prompt border** — `parseKeys` guards `buf.value.length > 0` before writing `\b \b`, but under some circumstances backspace still erases the prompt prefix and timestamp. Root cause not confirmed.
2. **Provider-specific rate-limit retry policy** — no retry logic for 429s from either provider.
3. **UI tests for `ui-raw.ts`** — `parseKeys` is tested; full integration-level UI tests