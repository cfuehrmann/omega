import { For, Show, ErrorBoundary, createEffect, onCleanup, createSignal, onMount } from "solid-js";
import { state, dispatch, type Turn, type WsEvent } from "./store";

/** Compile-time exhaustiveness guard for WsEvent switch in EventBlock. */
function exhaustiveCheck(x: never): null {
  console.warn("Unhandled WsEvent type:", (x as any).type);
  return null;
}

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

function truncate(s: string, maxChars = 3000): string {
  if (s.length <= maxChars) return s;
  return s.slice(0, maxChars) + `\n… [${s.length} chars total — showing first ${maxChars}]`;
}

function truncateOutput(s: string, maxLines = 5, maxChars = 500): string {
  const lines = s.split("\n");
  const linesCut = lines.length > maxLines;
  const charsCut = s.length > maxChars;
  if (!linesCut && !charsCut) return s;
  let result: string;
  let note: string;
  if (linesCut && (!charsCut || lines.slice(0, maxLines).join("\n").length <= maxChars)) {
    result = lines.slice(0, maxLines).join("\n");
    note = `… [${lines.length} lines / ${s.length} chars total — showing first ${maxLines} lines]`;
  } else {
    result = s.slice(0, maxChars);
    note = `… [${lines.length} lines / ${s.length} chars total — showing first ${maxChars} chars]`;
  }
  return result + "\n" + note;
}

function EventBlock(props: { event: WsEvent }) {
  const e = props.event;

  // Exhaustive switch over WsEvent — every variant must have a case.
  // Compile-time guard: if a new WsEvent variant is added without a render
  // case, TypeScript will error on the exhaustiveCheck(e) call in default.
  switch (e.type) {
    case "user_message":
      return (
        <div class="block user">
          <div class="block-label">user_message</div>
        </div>
      );

    case "text":
    case "assistant_text":
      return (
        <div class="block assist">
          <div class="block-label">assistant</div>
          <div class="block-body">{e.text}</div>
        </div>
      );

    case "tool_call": {
      const inputStr = typeof e.input === "object"
        ? JSON.stringify(e.input)
        : String(e.input);
      return (
        <div class="block tool">
          <div class="block-label">tool › {e.name}</div>
          <div class="block-body">{truncateOutput(inputStr)}</div>
        </div>
      );
    }

    case "tool_result": {
      const r = e.result;
      const content = r ? (r.type === "text" ? truncateOutput(r.text ?? "") : `[${r.type}]`) : "";
      return (
        <div class={`block result${r?.is_error ? " result-error" : ""}`}>
          <div class="block-label">result › {e.name}</div>
          <div class="block-body">{content}</div>
        </div>
      );
    }

    case "model_changed":
      return (
        <div class="block status">
          <div class="block-body">Switched to {e.provider} {e.model}</div>
        </div>
      );

    case "oauth_token_expired":
      return (
        <div class="block status">
          <div class="block-body">OAuth token expired/revoked — refreshing…</div>
        </div>
      );

    case "oauth_refreshed":
      return (
        <div class="block status">
          <div class="block-body">Token refreshed, retrying…</div>
        </div>
      );

    case "compact_user_start":
      return (
        <div class="block status">
          <div class="block-body">Compacting context…</div>
        </div>
      );

    case "compact_user_done": {
      const msg = e.messagesAfter === e.messagesBefore
        ? `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages (no change)`
        : `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <div class="block-body">{msg}</div>
        </div>
      );
    }

    case "compact_user_error":
      return (
        <div class="block error">
          <div class="block-body">⚠ Compaction failed: {e.error}</div>
        </div>
      );

    case "compact_auto_start":
      return (
        <div class="block status">
          <div class="block-body">Auto-compacting context ({e.messagesBefore} messages)…</div>
        </div>
      );

    case "compact_auto_done": {
      const msg = `Context auto-compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <div class="block-body">{msg}</div>
        </div>
      );
    }

    case "compact_auto_error":
      return (
        <div class="block error">
          <div class="block-body">⚠ Auto-compaction failed (rolling truncation fallback): {e.error}</div>
        </div>
      );

    case "world_state_saved":
      return (
        <div class="block world-state-saved">
          <div class="block-body">✓ world state saved ({e.charCount} chars)</div>
        </div>
      );

    case "llm_call": {
      const reqStr = e.request != null
        ? truncate(JSON.stringify(e.request, null, 2), 1000)
        : "(request not captured)";
      return (
        <details class="block api-call">
          <summary class="block-label">llm call › {e.provider}</summary>
          <div class="block-body">{reqStr}</div>
        </details>
      );
    }

    case "llm_response": {
      const u = e.usage;
      const parts = [
        `stop: ${e.stopReason}`,
        `in: ${u.input_tokens}`,
        `out: ${u.output_tokens}`,
        ...(u.cache_creation_input_tokens ? [`write: ${u.cache_creation_input_tokens}`] : []),
        ...(u.cache_read_input_tokens     ? [`read: ${u.cache_read_input_tokens}`]      : []),
        ...(u.service_tier && u.service_tier !== "standard" ? [`tier: ${u.service_tier}`] : []),
      ];
      return (
        <div class="block api-response">
          <div class="block-label">api response › {e.provider}</div>
          <div class="block-body">{parts.join("  ")}</div>
        </div>
      );
    }

    case "turn_end": {
      const m = e.metrics;
      const cost = m.costUsd != null ? `  cost: ${m.costUsd.toFixed(4)}` : "";
      const saved = m.savedUsd ? `  saved: ${m.savedUsd.toFixed(4)}` : "";
      const line = `in: ${m.inputTokens}  out: ${m.outputTokens}${cost}${saved}  model: ${e.model}`;
      return (
        <div class="block footer">
          <div class="block-body">{line}</div>
        </div>
      );
    }

    case "llm_error":
      return (
        <div class="block error-b">
          <div class="block-label">api error ({e.provider})</div>
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "agent_error":
      return (
        <div class="block error-b">
          <div class="block-label">error</div>
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "error":
      return (
        <div class="block error-b">
          <div class="block-label">error</div>
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "turn_interrupted":
      return <div class="block interrupt">⊘ Interrupted</div>;

    case "llm_retry":
      return (
        <div class="block info">
          <div class="block-label">llm retry (attempt {e.attempt})</div>
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "session_start":
      return (
        <div class="block info">
          <div class="block-label">session start</div>
          <div class="block-body">{e.authMode} · {e.provider} · {e.model}</div>
        </div>
      );

    case "session_end":
      return (
        <div class="block info">
          <div class="block-label">session end</div>
          <div class="block-body">{e.outcome}{e.reason ? ` — ${e.reason}` : ""}</div>
        </div>
      );

    // Web-protocol-only events — handled by dispatch(), never appear in turn.events.
    // Listed here to satisfy the exhaustive check.
    case "connected":
    case "disconnected":
    case "history":
    case "auth":
    case "turn_ready":
    case "reset_done":
      return null;

    default:
      // Compile-time exhaustiveness guard: TypeScript errors here if any
      // WsEvent variant is missing from the cases above.
      return exhaustiveCheck(e);
  }
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
        disabled={!state.connected || state.streaming}
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
    // Expose dispatch for e2e tests (harmless in production)
    (window as any).__omegaDispatch = dispatch;
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
        <ErrorBoundary fallback={(err) => (
          <div class="render-error">
            <strong>Render error</strong>
            <pre>{err?.message ?? String(err)}</pre>
          </div>
        )}>
          <For each={state.turns}>{(turn) => <TurnView turn={turn} />}</For>
        </ErrorBoundary>
      </div>
      <StatusDot />
      <InputArea />
    </div>
  );
}
