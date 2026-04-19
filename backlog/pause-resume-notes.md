# Working notes for pause-resume-interject implementation

## Task status

Executing `backlog/pause-resume-interject.md` — 4 stages, 4 commits.

- **Stage 1 DONE** — commit `91bda6b agent: add pause/resume/interject state machine`.
- **Stage 2 IN PROGRESS** — this session.
- Stages 3, 4 pending.

All development on `develop`. Last commits:
`91bda6b agent: add pause/resume/interject state machine`
`dceb413 plan: pause/resume/interject (replaces UX-1, UX-2)`

## STAGE 2 STATUS (in progress)

- `src/web/protocol.ts` — DONE: added `TurnStateSchema`/`TurnState`, `pause`/`continue` client messages, `turnState?` on `session_info`.
- `src/web/server.ts` — DONE: module state `currentTurnState`, `currentSessionName`; helpers `deriveTurnState`, `buildSessionInfo`, `setTurnState`; handlers for `pause`, `continue`; `abort` now calls `persistentAgent.requestAbort()`. reset/resume/rename/open all route through `buildSessionInfo`. The main `message` for-await loop calls `setTurnState(deriveTurnState(current, event))`. `pause_requested` is broadcast explicitly (not yielded by Agent).
- `src/web/client/state.ts` — DONE: added `turnState: TurnState` to AppState (init "idle"), updated in session_info (default "idle") and reset_done/handleDisconnect ("idle"). Added fallthrough cases for `pause_requested`/`turn_paused`/`turn_continued` that push to `events`.
- `src/web/pause-ws.test.ts` — DONE (writing). 3 of 4 tests passing.

### Failing test: "exposes turnState='paused' on a reconnecting socket"

Race: test sends `{type:"pause"}` immediately after `{type:"message"}` without first waiting for the server's turnState to flip to "running". The server's `pause` handler gates on `currentTurnState !== "running"` and silently drops the message if the turn hasn't started yet. Fix: add `await c1.waitFor(m => m.type === "session_info" && m.turnState === "running")` before sending pause. (Other two tests pass because they explicitly wait on `running` or `llm_call`.)

### After green gate

```
git add -A && git commit -m "server: wire pause/resume WS protocol and turnState"
```

STOP and report to user — don't start Stage 3.

## STAGE 2 PLAN (original)

### Key facts learned

- **`pause_requested` is NOT yielded by the Agent generator.** `requestPause()` only calls `void this.logEvent(ev)`. The server must explicitly broadcast the `pause_requested` event to WS clients after calling `agent.requestPause()`, since the for-await loop over `sendMessage` won't see it.
- **`turn_paused` / `turn_continued` ARE yielded** from the generator, so the server's for-await loop can derive turnState transitions from them.
- **`user_message` (interjection) is also yielded** from the generator between `turn_paused` and `turn_continued`.

### Files to modify

1. **`src/web/protocol.ts`**
   - Add `TurnStateSchema = z.enum(["idle", "running", "pause_requested", "paused"])` + `TurnState` type.
   - Add `ClientMessage` variants: `{type:"pause"}` and `{type:"continue", content?: string}`.
   - Add `turnState: TurnStateSchema.optional()` to `session_info` schema and the `satisfies` type list.

2. **`src/web/server.ts`**
   - Add module-scoped `currentTurnState: TurnState = "idle"`.
   - Add helper `deriveTurnState(prev, event): TurnState`:
     - `user_message` → `running`
     - `pause_requested` → `pause_requested`
     - `turn_paused` → `paused`
     - `turn_continued` → `running`
     - `turn_end` / `turn_interrupted` → `idle`
     - default → unchanged
   - Add helper `setTurnState(next)` that updates the module var + broadcasts session_info if changed. Must **not** broadcast when unchanged.
   - In the `message` handler's `for await (event of sendMessage(...))` loop, apply `deriveTurnState` to every yielded event and call `setTurnState`.
   - Add new handlers:
     - `pause` → `agent.requestPause()`, then **explicitly broadcast** `{type:"pause_requested", time: now()}` via `broadcast()` (since not yielded), then `setTurnState("pause_requested")`.
     - `continue` → `agent.requestContinue(msg.content)`. The yielded events (user_message, turn_continued) will drive turnState → running via the for-await loop.
     - Convert existing `abort` handler to use `agent.requestAbort()` instead of `activeAbortController?.abort()`. (Both should work since agent merges external signal with internal controller, but requestAbort is the documented contract.)
   - On `reset`: set `currentTurnState = "idle"` (no broadcast, since session_info is sent in the cork).
   - On `resume_session`: set `currentTurnState = "idle"` on completion (resumption is not a user-visible turn).
   - Include `turnState: currentTurnState` in every `session_info` message:
     - WS open (existing session, reconnect)
     - `reset` cork
     - `resume_session` cork
     - Fresh `setTurnState` broadcasts

3. **`src/web/client/state.ts`**
   - Add `turnState: TurnState` to `AppState` (import from `../protocol`).
   - Initialize to `"idle"`.
   - In `session_info` case: `setState("turnState", event.turnState ?? "idle")`.
   - In `reset_done` case: `setState("turnState", "idle")`.
   - In `handleDisconnect`: `setState("turnState", "idle")` (no longer known).
   - Add cases for `pause_requested`, `turn_paused`, `turn_continued` that push the event to `state.events` (Stage 3 will render them). The switch currently has default: break at lines ~361 and ~443 inside dispatch helpers (computeDurations/computeLiveDurations), but `dispatch` itself doesn't have a default — still, those events need to land in `state.events` so the feed shows them eventually.

4. **Tests**
   New test file (suggested): `src/web/pause-ws.test.ts`:
   - Test 1: WS `pause` → server invokes `agent.requestPause` (spy on the method).
   - Test 2: Reconnect during `paused` shows `turnState: "paused"` in next session_info.
   - Test 3: Reconnect during `pause_requested` shows `turnState: "pause_requested"`. Use a mock stream whose `finalMessage()` blocks on a test-owned promise so we get a deterministic window.
   - Test 4: `turnState` transitions emitted: `running` → `pause_requested` → `paused` → `running`.

### Test mechanics

- Use `runWebApp({port, streamProvider, sessionsRoot: TEST_SESSIONS_ROOT})` pattern.
- Monkey-patch `Bun.serve` to capture the `bunServer` handle for clean shutdown (see `src/web/reset-init-events.test.ts` for the pattern).
- Use the mock stream helpers from `src/agent-pause.test.ts` (toolUseStreamEvents, toolUseMessage, textStreamEvents, textMessage, makeMockStream).
- For the pause-requested window in Test 3, create a provider whose first call's `finalMessage` awaits a promise the test resolves later.
- Be careful: `ClientMessageSchema.parse()` in server requires the message schema to match exactly. Send `{type: "pause"}` etc.
- After each test, close WS + stop server + give 20ms for cleanup.

### Commit

After gate passes:
```
git add -A && git commit -m "server: wire pause/resume WS protocol and turnState"
```

**STOP and report** after the commit.

## Stages 3–4 to do (do NOT start without user's go-ahead)
- Stage 3: `web: render pause/resume UI and keyboard shortcuts`
- Stage 4: `e2e: pause/resume integration tests; retire UX-1/UX-2`

## Key file locations (verified)

- `src/events.ts` — OmegaEvent union
- `src/events.schema.ts` — OmegaEventSchema discriminatedUnion; `parseOmegaEvent` returns safeParse
- `src/agent.ts` — class at line ~366, pause methods ~458-500, sendMessage ~980, pause seam ~1555
- `src/agent-pause.test.ts` — stage 1 tests, useful patterns for mock streams
- `src/web/protocol.ts` — ClientMessage, ServerMessage, OmegaModelSchema, OmegaEffortSchema
- `src/web/server.ts` — handlers, module state: `persistentAgent`, `currentSessionPaths`, `activeSession`, `isStreaming`, `activeAbortController`
- `src/web/client/state.ts` — AppState interface ~106, createStore ~155, dispatch switch ~285+, session_info case ~605, reset_done ~613
- `src/web/client/App.tsx` — EventBlock switch line ~675; pause_requested/turn_paused/turn_continued cases added at line ~1088 returning null (Stage 1)
- `src/web/reset-init-events.test.ts` — reference pattern for server/WS tests
- `src/test-utils.ts` — `makeTestAgent(provider?)`: returns `{agent, sessionDir, contextFile, eventsFile, dispose}`

## Workflow rules
- Commit with `git add -A && git commit -m "..."`, exit 0 = gate passed. Pre-commit runs `just gate`.
- Never use --no-verify.
- After each stage: STOP and report before starting next.

## Stage commits (per plan)
- Stage 1 (done): `agent: add pause/resume/interject state machine`
- Stage 2: `server: wire pause/resume WS protocol and turnState`
- Stage 3: `web: render pause/resume UI and keyboard shortcuts`
- Stage 4: `e2e: pause/resume integration tests; retire UX-1/UX-2`
