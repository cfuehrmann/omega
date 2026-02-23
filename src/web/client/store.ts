/**
 * Application store — Turn[] state model.
 *
 * All UI rendering derives from this reactive store.
 * Each Turn holds an ordered list of AgentEvent-shaped objects
 * received over the WebSocket.
 */

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
  | { type: "user_message"; content: string }
  | { type: "text"; text: string }
  | { type: "tool_call"; id: string; name: string; input: unknown }
  | { type: "tool_result"; id: string; name: string; result: { type: string; text?: string; is_error?: boolean } }
  | { type: "status"; message: string }
  | { type: "api_call_start"; callNumber: number; provider: string; url: string; request: unknown }
  | { type: "api_response"; provider: string; url: string; stopReason: string; usage: { input_tokens: number; output_tokens: number }; content: unknown[] }
  | { type: "world_state_saved"; path: string; charCount: number }
  | { type: "turn_end"; metrics: { inputTokens: number; outputTokens: number; costUsd: number; savedUsd?: number; ttftMs: number | null }; model: string; provider: string }
  | { type: "api_error"; provider: string; error: string }
  | { type: "error"; error: string }
  | { type: "interrupted" };

export interface Turn {
  id: number;
  events: WsEvent[];
  /** Accumulated streaming text for the current assistant message */
  streamingText: string;
  done: boolean;
}

export interface AppState {
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
      // Replay all logged events to rebuild the store from scratch.
      // Reset state first so we don't duplicate on reconnect.
      setState("turns", []);
      setState("authMode", "");
      setState("streaming", false);
      nextTurnId = 0;
      for (const e of event.events) {
        dispatch(e);
      }
      // After replay we are still connected (history arrived over an open socket)
      setState("connected", true);
      setState("retryCount", 0);
      break;
    }

    case "auth":
      setState("authMode", event.mode);
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

    case "interrupted":
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

    case "tool_call":
    case "tool_result":
    case "status":
    case "api_call_start":
    case "api_response":
    case "world_state_saved":
    case "api_error":
    case "error":
      appendEvent(event);
      break;
  }
}
