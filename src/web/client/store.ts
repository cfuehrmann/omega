/**
 * Application store — Turn[] state model.
 *
 * All UI rendering derives from this reactive store.
 * Each Turn holds an ordered list of AgentEvent-shaped objects
 * received over the WebSocket.
 */

import { batch } from "solid-js";
import { createStore, produce } from "solid-js/store";

// ---------------------------------------------------------------------------
// Types mirroring src/agent.ts AgentEvent (subset we care about for display)
// ---------------------------------------------------------------------------

export type WsEvent =
  | { type: "connected" }
  | { type: "disconnected" }
  | { type: "history"; events: WsEvent[] }
  | { type: "auth"; mode: string }
  | { type: "turn_ready" }
  | { type: "reset_done" }
  | { type: "user_message"; content: string }
  | { type: "text"; text: string }
  | { type: "assistant_text"; text: string }
  // OmegaEvent variants (persisted names are authoritative — see plan/dev-policy.md)
  | { type: "session_start"; authMode: string; model: string; provider: string; systemPrompt: string }
  | { type: "session_end"; outcome: "clean" | "error"; reason?: string }
  | { type: "tool_call"; id: string; name: string; input: unknown; formatted?: string }
  | { type: "tool_result"; id: string; name: string; result?: { type: string; text?: string; is_error?: boolean }; formatted?: string; isError: boolean }
  | { type: "llm_response"; provider: string; url: string; stopReason: string; usage: { input_tokens: number; output_tokens: number; cache_creation_input_tokens?: number | null; cache_read_input_tokens?: number | null; service_tier?: string | null }; content?: unknown[]; raw?: unknown }
  | { type: "llm_call"; provider: string; url: string; model: string; contextHashes: string[]; cacheBreakpointIndex: number | null; request?: unknown }
  | { type: "llm_retry"; attempt: number; provider: string; waitMs: number; error: string }
  | { type: "model_changed"; provider: string; model: string }
  | { type: "oauth_token_expired"; attempt: number; httpStatus?: number }
  | { type: "oauth_refreshed" }
  | { type: "compact_user_start" }
  | { type: "compact_user_done"; messagesBefore: number; messagesAfter: number }
  | { type: "compact_user_error"; error: string }
  | { type: "compact_auto_start"; messagesBefore: number }
  | { type: "compact_auto_done"; messagesBefore: number; messagesAfter: number }
  | { type: "compact_auto_error"; error: string }
  | { type: "world_state_saved"; path: string; charCount: number }
  | { type: "turn_end"; metrics: { inputTokens: number; outputTokens: number; costUsd: number; savedUsd?: number; ttftMs: number | null }; model: string; provider: string }
  | { type: "llm_error"; provider: string; error: string }
  | { type: "agent_error"; error: string }
  | { type: "error"; error: string }
  | { type: "turn_interrupted" };

export interface Turn {
  id: number;
  events: WsEvent[];
  /** Accumulated streaming text for the current assistant message */
  streamingText: string;
  done: boolean;
}

interface AppState {
  connected: boolean;
  streaming: boolean;
  authMode: string;
  turns: Turn[];
  /** Number of consecutive failed reconnect attempts (reset on successful connect) */
  retryCount: number;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

const [state, setState] = createStore<AppState>({
  connected: false,
  streaming: false,
  authMode: "",
  turns: [],
  retryCount: 0,
});

export { state };

let nextTurnId = 0;

function currentTurn(): Turn | undefined {
  return state.turns[state.turns.length - 1];
}

function appendEvent(event: WsEvent): void {
  setState(produce(s => {
    const turn = s.turns[s.turns.length - 1];
    if (turn) turn.events.push(event);
  }));
}

export function dispatch(event: WsEvent): void {
  switch (event.type) {
    case "connected":
      setState("connected", true);
      setState("streaming", false);
      setState("retryCount", 0);
      break;

    case "disconnected":
      setState("connected", false);
      setState("streaming", false);
      setState("retryCount", r => r + 1);
      break;

    case "history": {
      // Rebuild turns from the event log using plain JS objects, then set the
      // final result in one store write.  Previous approach dispatched each
      // event individually inside batch(), mixing array replacement with
      // produce-based mutation — this confused SolidJS's fine-grained change
      // tracking and left <For> with a stale reference after a page reload.
      const turns: Turn[] = [];
      let streaming = false;
      let tid = 0;

      for (const e of event.events) {
        const cur = () => turns[turns.length - 1];

        switch (e.type) {
          case "user_message":
            turns.push({ id: tid++, events: [e], streamingText: "", done: false });
            streaming = true;
            break;

          case "assistant_text": {
            const t = cur();
            if (t && !t.streamingText) {
              t.events.push({ type: "text", text: (e as any).text });
            }
            break;
          }

          case "turn_end": {
            const t = cur();
            if (t) {
              if (t.streamingText) {
                t.events.push({ type: "text", text: t.streamingText });
                t.streamingText = "";
              }
              t.events.push(e);
              t.done = true;
            }
            streaming = false;
            break;
          }

          case "turn_interrupted": {
            const t = cur();
            if (t) {
              if (t.streamingText) {
                t.events.push({ type: "text", text: t.streamingText });
                t.streamingText = "";
              }
              t.events.push(e);
              t.done = true;
            }
            streaming = false;
            break;
          }

          // Transport-only events — never appear in persisted history,
          // but listed for completeness (server filters them out).
          case "connected":
          case "disconnected":
          case "history":
          case "auth":
          case "turn_ready":
          case "reset_done":
            break;

          // All other events: append to the current turn's event list.
          default: {
            const t = cur();
            if (t) t.events.push(e);
            break;
          }
        }
      }

      // Close any open turn (server crashed mid-turn, no turn_end emitted).
      if (streaming) {
        const t = turns[turns.length - 1];
        if (t) {
          if (t.streamingText) {
            t.events.push({ type: "text", text: t.streamingText });
            t.streamingText = "";
          }
          t.events.push({ type: "turn_interrupted" });
          t.done = true;
        }
        streaming = false;
      }

      nextTurnId = tid;
      // Single batch write — SolidJS sees one atomic state transition.
      batch(() => {
        setState("turns", turns);
        setState("authMode", "");
        setState("streaming", false);
        setState("connected", true);
        setState("retryCount", 0);
      });
      break;
    }

    case "auth":
      setState("authMode", event.mode);
      break;

    case "reset_done":
      // Server has created a new agent — clear all UI state
      setState("turns", []);
      setState("streaming", false);
      nextTurnId = 0;
      break;

    case "user_message":
      // Start a new turn
      setState("turns", t => [...t, {
        id: nextTurnId++,
        events: [event],
        streamingText: "",
        done: false,
      }]);
      setState("streaming", true);
      break;

    case "text":
      setState(produce(s => {
        const turn = s.turns[s.turns.length - 1];
        if (turn) turn.streamingText += event.text;
      }));
      break;

    case "assistant_text":
      // Persisted full-text event — used during history replay.
      // During live streaming, streamingText is already populated from `text` fragments;
      // we skip appending here to avoid duplication. During replay, streamingText is
      // empty (no `text` fragments), so we push a synthetic text block into events.
      setState(produce(s => {
        const turn = s.turns[s.turns.length - 1];
        if (!turn) return;
        if (!turn.streamingText) {
          // Replay path: no live fragments — push the full text as a rendered block.
          turn.events.push({ type: "text", text: event.text });
        }
        // Live path: streamingText already has it; turn_end will freeze it — no-op.
      }));
      break;

    case "turn_end":
      setState(produce(s => {
        const turn = s.turns[s.turns.length - 1];
        if (turn) {
          // Freeze streaming text into the event list
          if (turn.streamingText) {
            turn.events.push({ type: "text", text: turn.streamingText });
            turn.streamingText = "";
          }
          turn.events.push(event);
          turn.done = true;
        }
      }));
      // turn_end means the agentic loop finished; clear streaming so replayed
      // history doesn't leave the UI stuck in streaming state.
      // turn_ready (which also clears streaming) is excluded from replay.
      setState("streaming", false);
      break;

    case "turn_ready":
      setState("streaming", false);
      break;

    case "turn_interrupted":
      setState(produce(s => {
        const turn = s.turns[s.turns.length - 1];
        if (turn) {
          if (turn.streamingText) {
            turn.events.push({ type: "text", text: turn.streamingText });
            turn.streamingText = "";
          }
          turn.events.push(event);
          turn.done = true;
        }
      }));
      setState("streaming", false);
      break;

    case "session_start":
    case "session_end":
    case "tool_call":
    case "tool_result":
    case "llm_response":
    case "llm_call":
    case "llm_retry":
    case "model_changed":
    case "oauth_token_expired":
    case "oauth_refreshed":
    case "compact_user_start":
    case "compact_user_done":
    case "compact_user_error":
    case "compact_auto_start":
    case "compact_auto_done":
    case "compact_auto_error":
    case "world_state_saved":
    case "llm_error":
    case "agent_error":
    case "error":
      appendEvent(event);
      break;
  }
}
