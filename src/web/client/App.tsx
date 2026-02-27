import { For, Show, createEffect, onCleanup, createSignal, onMount } from "solid-js";
import { state, dispatch, type Turn, type WsEvent } from "./store";

// ---------------------------------------------------------------------------
// WebSocket (module-level state, initialised inside App via startWs())
// ---------------------------------------------------------------------------

let ws: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let connectAttempts = 0;

function connect() {
  connectAttempts++;
  // Connect to the same host:port that served the page.
  // In dev mode (Vite on :5173), the /ws path is proxied to the Bun server.
  // In production and tests, the Bun server serves both HTTP and WebSocket.
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = `${proto}//${location.host}/ws`;
  ws = new WebSocket(wsUrl);
  // Expose for e2e tests (harmless in production)
  (window as any).__omegaWs = ws;

  ws.onopen = () => {
    connectAttempts = 0;
    dispatch({ type: "connected" });
  };

  ws.onerror = () => {
    // onclose fires right after onerror — let onclose handle the dispatch
  };

  ws.onmessage = (e) => {
    let event: WsEvent;
    try { event = JSON.parse(e.data as string); } catch { return; }
    // Skip redundant server-sent "connected" — we already handled it in onopen
    if (event.type === "connected") return;
    dispatch(event);
  };

  ws.onclose = () => {
    dispatch({ type: "disconnected" });
    reconnectTimer = setTimeout(connect, 2000);
  };
}

function sendToServer(msg: object) {
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}

// ---------------------------------------------------------------------------
// Event block components
// ---------------------------------------------------------------------------

function truncate(s: string, max = 3000): string {
  if (s.length <= max) return s;
  return s.slice(0, max) + `\n… [${s.length - max} chars truncated]`;
}

function EventBlock(props: { event: WsEvent }) {
  const e = props.event;

  if (e.type === "user_message") {
    return (
      <div class="block user">
        <div class="block-label">you</div>
        <div class="block-body">{e.content}</div>
      </div>
    );
  }

  if (e.type === "text") {
    return (
      <div class="block assist">
        <div class="block-label">assistant</div>
        <div class="block-body">{e.text}</div>
      </div>
    );
  }

  if (e.type === "agent_to_agent_tool_call") {
    const inputStr = typeof e.input === "object"
      ? JSON.stringify(e.input, null, 2)
      : String(e.input);
    return (
      <div class="block tool">
        <div class="block-label">tool › {e.name}</div>
        <div class="block-body">{truncate(inputStr)}</div>
      </div>
    );
  }

  if (e.type === "agent_to_agent_tool_result") {
    const r = e.result;
    const content = r.type === "text" ? truncate(r.text ?? "") : `[${r.type}]`;
    return (
      <div class={`block result${r.is_error ? " result-error" : ""}`}>
        <div class="block-label">result › {e.name}</div>
        <div class="block-body">{content}</div>
      </div>
    );
  }

  if (e.type === "status") {
    return (
      <div class="block status">
        <div class="block-body">{e.message}</div>
      </div>
    );
  }

  if (e.type === "world_state_saved") {
    return (
      <div class="block world-state-saved">
        <div class="block-body">✓ world state saved ({e.charCount} chars)</div>
      </div>
    );
  }

  if (e.type === "llm_call") {
    const reqStr = truncate(JSON.stringify(e.request, null, 2), 1000);
    return (
      <details class="block api-call">
        <summary class="block-label">api call #{e.callNumber} › {e.provider}</summary>
        <div class="block-body">{reqStr}</div>
      </details>
    );
  }

  if (e.type === "llm_to_agent") {
    const line = `stop: ${e.stopReason}  in: ${e.usage.input_tokens}  out: ${e.usage.output_tokens}`;
    return (
      <div class="block api-response">
        <div class="block-label">api response › {e.provider}</div>
        <div class="block-body">{line}</div>
      </div>
    );
  }

  if (e.type === "turn_end") {
    const m = e.metrics;
    const cost = m.costUsd != null ? `  cost: $${m.costUsd.toFixed(4)}` : "";
    const saved = m.savedUsd ? `  saved: $${m.savedUsd.toFixed(4)}` : "";
    const line = `in: ${m.inputTokens}  out: ${m.outputTokens}${cost}${saved}  model: ${e.model}`;
    return (
      <div class="block footer">
        <div class="block-body">{line}</div>
      </div>
    );
  }

  if (e.type === "llm_error") {
    return (
      <div class="block error-b">
        <div class="block-label">api error ({e.provider})</div>
        <div class="block-body">{e.error}</div>
      </div>
    );
  }

  if (e.type === "agent_error" || e.type === "error") {
    return (
      <div class="block error-b">
        <div class="block-label">error</div>
        <div class="block-body">{e.error}</div>
      </div>
    );
  }

  if (e.type === "turn_interrupted") {
    return <div class="block interrupt">⊘ Interrupted</div>;
  }

  return null;
}

function TurnView(props: { turn: Turn }) {
  return (
    <div class="turn">
      <For each={props.turn.events}>{(event) => <EventBlock event={event} />}</For>
      <Show when={props.turn.streamingText}>
        <div class="block assist streaming">
          <div class="block-label">assistant</div>
          <div class="block-body">
            {props.turn.streamingText}
            <span class="cursor" />
          </div>
        </div>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

function InputArea() {
  let textareaRef!: HTMLTextAreaElement;

  const [inputValue, setInputValue] = createSignal("");

  function autoResize() {
    textareaRef.style.height = "auto";
    textareaRef.style.height = Math.min(textareaRef.scrollHeight, 200) + "px";
  }

  function send() {
    const content = inputValue().trim();
    if (!content || state.streaming || !state.connected) return;
    sendToServer({ type: "message", content });
    setInputValue("");
    setTimeout(autoResize, 0);
  }

  function abort() {
    sendToServer({ type: "abort" });
  }

  function newSession() {
    if (!state.connected || state.streaming) return;
    if (!confirm("Start a new session? This will clear all history.")) return;
    sendToServer({ type: "reset" });
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
    setTimeout(autoResize, 0);
  }

  return (
    <div class="input-area">
      <textarea
        ref={textareaRef}
        value={inputValue()}
        onInput={(e) => { setInputValue(e.currentTarget.value); autoResize(); }}
        onKeyDown={onKeyDown}
        placeholder="Message Omega… (Enter to send, Shift+Enter for newline)"
        rows={1}
        disabled={!state.connected}
      />
      <Show when={state.streaming}
        fallback={
          <div class="btn-group">
            <button class="send-btn" onClick={send} disabled={!state.connected}>
              Send
            </button>
            <Show when={state.connected && state.turns.length > 0}>
              <button class="new-session-btn" onClick={newSession} title="Start a new session (clears history)">
                ＋ New
              </button>
            </Show>
          </div>
        }
      >
        <button class="abort-btn" onClick={abort}>Abort</button>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Reconnection banner
// ---------------------------------------------------------------------------

function ReconnectBanner() {
  // Show after 2+ consecutive disconnects (i.e. at least one retry has failed)
  return (
    <Show when={!state.connected && state.retryCount >= 2}>
      <div class="reconnect-banner">
        ⚠ Cannot reach server — retrying… (attempt {state.retryCount})
        <br />
        Run <code>just server</code> in a terminal, then this page will reconnect automatically.
      </div>
    </Show>
  );
}

// ---------------------------------------------------------------------------
// Header / status
// ---------------------------------------------------------------------------

function StatusDot() {
  const cls = () =>
    !state.connected ? "dot error"
    : state.streaming  ? "dot streaming"
    : "dot connected";

  const label = () =>
    !state.connected ? "disconnected"
    : state.streaming  ? "streaming…"
    : "ready";

  return (
    <div class="status-row">
      <span class={cls()} />
      <h1>Ω Omega</h1>
      <span class="status-label">{label()}</span>
      <span class="model-label">{state.authMode}</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Root App
// ---------------------------------------------------------------------------

export function App() {
  let feedRef!: HTMLDivElement;

  // Start WebSocket on mount, clean up on unmount
  onMount(() => {
    connect();
  });
  onCleanup(() => {
    ws?.close();
    if (reconnectTimer) clearTimeout(reconnectTimer);
  });

  // Auto-scroll to bottom on new content
  createEffect(() => {
    // Access turns length to track changes
    const _ = state.turns.length;
    const lastTurn = state.turns[state.turns.length - 1];
    const __ = lastTurn?.streamingText;
    queueMicrotask(() => {
      if (feedRef) feedRef.scrollTop = feedRef.scrollHeight;
    });
  });

  return (
    <div class="app">
      <ReconnectBanner />
      <div class="feed" ref={feedRef}>
        <For each={state.turns}>{(turn) => <TurnView turn={turn} />}</For>
      </div>
      <StatusDot />
      <InputArea />
    </div>
  );
}
