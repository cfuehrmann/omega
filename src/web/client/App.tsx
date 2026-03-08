import { For, Show, ErrorBoundary, createEffect, onCleanup, createSignal, onMount, createMemo } from "solid-js";
import { state, dispatch, type Turn, type WsEvent, type StickyMetrics } from "./store";

/** Compile-time exhaustiveness guard for WsEvent switch in EventBlock. */
function exhaustiveCheck(x: never): null {
  console.warn("Unhandled WsEvent type:", (x as any).type);
  return null;
}

/**
 * Format an ISO timestamp string to local date + time with millisecond
 * precision: "YYYY-MM-DD HH:mm:ss.mmm". Returns "" when ts is absent.
 */
function formatTs(ts: string | undefined): string {
  if (!ts) return "";
  const d = new Date(ts);
  if (isNaN(d.getTime())) return ts; // fallback: show raw string
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`
  );
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

/** First non-empty line of a string, for collapsed previews. */
function firstLine(s: string): string {
  return s.split("\n").find(l => l.trim()) ?? s.slice(0, 80);
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

interface ToolDetail {
  name: string;
  input: unknown;
  output: string;
  isError: boolean;
  durationMs: number;
}

interface LlmCallDetail {
  provider: string;
  url: string;
  model: string;
  contextHashes: string[];
  request: any;
}

interface LlmResponseDetail {
  provider: string;
  url: string;
  model: string;
  stopReason: string;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens?: number | null;
    cache_read_input_tokens?: number | null;
    service_tier?: string | null;
  };
  content: any;
  raw: any;
}

type ModalContent =
  | { kind: "tool"; detail: ToolDetail }
  | { kind: "llm_call"; detail: LlmCallDetail }
  | { kind: "llm_response"; detail: LlmResponseDetail };

const [activeModal, setActiveModal] = createSignal<ModalContent | null>(null);

// Keep the old setToolModal helper so tool_call/tool_result call sites are unchanged
function setToolModal(d: ToolDetail | null) {
  setActiveModal(d ? { kind: "tool", detail: d } : null);
}

function closeModal() { setActiveModal(null); }

function ModalShell(props: { title: string; cls: string; children: any }) {
  return (
    <div class="modal-backdrop">
      <div class={`modal ${props.cls}`}>
        <div class="modal-header">
          <span class="modal-title">{props.title}</span>
          <button class="modal-close" onClick={closeModal}>✕ close</button>
        </div>
        {props.children}
      </div>
    </div>
  );
}

function ActiveModal() {
  return (
    <Show when={activeModal()}>
      {(m) => {
        const modal = m();
        if (modal.kind === "tool") {
          const d = modal.detail;
          return (
            <ModalShell title={`tool › ${d.name}`} cls="tool-modal">
              <div class="modal-section-label">input</div>
              <pre class="modal-body">{
                d.input == null
                  ? "(none)"
                  : typeof d.input === "object"
                    ? JSON.stringify(d.input, null, 2)
                    : String(d.input)
              }</pre>
              <div class="modal-section-label">
                output
                <span class="modal-meta">
                  {d.isError ? " · error" : ""}
                  {" · "}{d.durationMs.toFixed(0)} ms
                </span>
              </div>
              <pre class="modal-body">{d.output}</pre>
            </ModalShell>
          );
        }
        if (modal.kind === "llm_call") {
          const d = modal.detail;
          const reqStr = d.request != null
            ? JSON.stringify(d.request, null, 2)
            : "(request not captured)";
          return (
            <ModalShell title={`llm_call › ${d.provider}`} cls="llm-call-modal">
              <div class="modal-section-label">
                {d.model}
                <span class="modal-meta"> · {d.url} · {d.contextHashes.length} context messages</span>
              </div>
              <pre class="modal-body">{reqStr}</pre>
            </ModalShell>
          );
        }
        // llm_response
        const d = modal.detail;
        const u = d.usage;
        const usageParts = [
          `input_tokens: ${u.input_tokens}`,
          `output_tokens: ${u.output_tokens}`,
          ...(u.cache_creation_input_tokens ? [`cache_write: ${u.cache_creation_input_tokens}`] : []),
          ...(u.cache_read_input_tokens     ? [`cache_read: ${u.cache_read_input_tokens}`]      : []),
          ...(u.service_tier && u.service_tier !== "standard" ? [`service_tier: ${u.service_tier}`] : []),
        ].join("  ");
        const contentStr = d.content != null
          ? JSON.stringify(d.content, null, 2)
          : d.raw != null
            ? JSON.stringify(d.raw, null, 2)
            : "(content not captured)";
        return (
          <ModalShell title={`llm_response › ${d.provider}`} cls="llm-resp-modal">
            <div class="modal-section-label">
              {d.model}
              <span class="modal-meta"> · stop: {d.stopReason} · {d.url}</span>
            </div>
            <div class="modal-section-label">usage<span class="modal-meta"> · {usageParts}</span></div>
            <pre class="modal-body">{contentStr}</pre>
          </ModalShell>
        );
      }}
    </Show>
  );
}

// ---------------------------------------------------------------------------
// Event block
// ---------------------------------------------------------------------------

/** Timestamp line rendered inside every block that carries a ts field. */
function BlockTs(props: { ts?: string }) {
  const label = () => formatTs(props.ts);
  return <Show when={label()}><div class="block-ts">{label()}</div></Show>;
}

function EventBlock(props: { event: WsEvent; turnEvents: WsEvent[]; streaming?: boolean }) {
  const e = props.event;
  const ts = (e as any).ts as string | undefined;

  // Exhaustive switch over WsEvent — every variant must have a case.
  // Compile-time guard: if a new WsEvent variant is added without a render
  // case, TypeScript will error on the exhaustiveCheck(e) call in default.
  switch (e.type) {
    case "user_message":
      return (
        <div class="block user">
          <div class="block-label">user_message</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.content}</div>
        </div>
      );

    case "text":
    case "assistant_text":
      return (
        <div class={`block assist${props.streaming ? " streaming" : ""}`}>
          <div class="block-label">assistant</div>
          <BlockTs ts={ts} />
          <div class="block-body">
            {e.text}
            <Show when={props.streaming}><span class="cursor" /></Show>
          </div>
        </div>
      );

    case "tool_call": {
      const inputPreview = createMemo(() => {
        if (e.input == null) return "(none)";
        const s = typeof e.input === "object" ? JSON.stringify(e.input) : String(e.input);
        return firstLine(s);
      });
      // Find the matching tool_result in the same turn by id
      const result = createMemo(() =>
        props.turnEvents.find(
          (ev): ev is WsEvent & { type: "tool_result" } =>
            ev.type === "tool_result" && (ev as any).id === e.id
        )
      );
      const openModal = () => {
        const r = result();
        setToolModal({
          name: e.name,
          input: e.input,
          output: r ? r.output : "(not yet available)",
          isError: r ? r.isError : false,
          durationMs: r ? r.durationMs : 0,
        });
      };
      return (
        <div class="block tool">
          <div class="block-label-row">
            <span class="block-label">tool › {e.name}</span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
          <BlockTs ts={ts} />
          <div class="block-body block-preview">{inputPreview()}</div>
        </div>
      );
    }

    case "tool_result": {
      const outputPreview = createMemo(() => firstLine(e.output));
      // Find matching tool_call for the modal
      const call = props.turnEvents.find(
        (ev): ev is WsEvent & { type: "tool_call" } =>
          ev.type === "tool_call" && (ev as any).id === e.id
      );
      const openModal = () => {
        setToolModal({
          name: e.name,
          input: call ? call.input : null,
          output: e.output,
          isError: e.isError,
          durationMs: e.durationMs,
        });
      };
      return (
        <div class={`block result${e.isError ? " result-error" : ""}`}>
          <div class="block-label-row">
            <span class="block-label">result › {e.name}</span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
          <BlockTs ts={ts} />
          <div class="block-body block-preview">{outputPreview()}</div>
        </div>
      );
    }

    case "model_changed":
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">Switched to {e.provider} {e.model}</div>
        </div>
      );

    case "oauth_token_expired":
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">OAuth token expired/revoked — refreshing…</div>
        </div>
      );

    case "oauth_refreshed":
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">Token refreshed, retrying…</div>
        </div>
      );

    case "compact_user_start":
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">Compacting context…</div>
        </div>
      );

    case "compact_user_done": {
      const msg = e.messagesAfter === e.messagesBefore
        ? `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages (no change)`
        : `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">{msg}</div>
        </div>
      );
    }

    case "compact_user_error":
      return (
        <div class="block error">
          <BlockTs ts={ts} />
          <div class="block-body">⚠ Compaction failed: {e.error}</div>
        </div>
      );

    case "compact_auto_start":
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">Auto-compacting context ({e.messagesBefore} messages)…</div>
        </div>
      );

    case "compact_auto_done": {
      const msg = `Context auto-compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <BlockTs ts={ts} />
          <div class="block-body">{msg}</div>
        </div>
      );
    }

    case "compact_auto_error":
      return (
        <div class="block error">
          <BlockTs ts={ts} />
          <div class="block-body">⚠ Auto-compaction failed (rolling truncation fallback): {e.error}</div>
        </div>
      );

    case "world_state_saved":
      return (
        <div class="block world-state-saved">
          <BlockTs ts={ts} />
          <div class="block-body">✓ world state saved ({e.charCount} chars)</div>
        </div>
      );

    case "llm_call": {
      const openModal = () => setActiveModal({
        kind: "llm_call",
        detail: {
          provider: e.provider,
          url: e.url,
          model: e.model,
          contextHashes: e.contextHashes,
          request: e.request,
        },
      });
      return (
        <div class="block api-call">
          <div class="block-label-row">
            <span class="block-label">llm_call › {e.provider}</span>
            <button class="block-expand-btn" onClick={openModal} title="View full request">⤢</button>
          </div>
          <BlockTs ts={ts} />
          <div class="block-body block-preview">{e.model} · {e.contextHashes.length} messages</div>
        </div>
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
      const openModal = () => setActiveModal({
        kind: "llm_response",
        detail: {
          provider: e.provider,
          url: e.url,
          model: e.model,
          stopReason: e.stopReason,
          usage: e.usage,
          content: e.content,
          raw: e.raw,
        },
      });
      return (
        <div class="block api-response">
          <div class="block-label-row">
            <span class="block-label">llm_response › {e.provider}</span>
            <button class="block-expand-btn" onClick={openModal} title="View full response">⤢</button>
          </div>
          <BlockTs ts={ts} />
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
          <BlockTs ts={ts} />
          <div class="block-body">{line}</div>
        </div>
      );
    }

    case "llm_error":
      return (
        <div class="block error-b">
          <div class="block-label">api error ({e.provider})</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "agent_error":
      return (
        <div class="block error-b">
          <div class="block-label">error</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "error":
      return (
        <div class="block error-b">
          <div class="block-label">error</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "turn_interrupted":
      return (
        <div class="block interrupt">
          <BlockTs ts={ts} />
          ⊘ Interrupted
        </div>
      );

    case "llm_retry":
      return (
        <div class="block info">
          <div class="block-label">llm retry (attempt {e.attempt})</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.error}</div>
        </div>
      );

    case "session_start":
      return (
        <div class="block info">
          <div class="block-label">session start</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.authMode} · {e.provider} · {e.model}</div>
        </div>
      );

    case "session_end":
      return (
        <div class="block info">
          <div class="block-label">session end</div>
          <BlockTs ts={ts} />
          <div class="block-body">{e.outcome}{e.reason ? ` — ${e.reason}` : ""}</div>
        </div>
      );

    // Web-protocol-only events — handled by dispatch(), never appear in turn.events.
    // Listed here to satisfy the exhaustive check.
    case "connected":
    case "disconnected":
    case "history":
    case "auth":
    case "reset_done":
      return null;

    default:
      // Compile-time exhaustiveness guard: TypeScript errors here if any
      // WsEvent variant is missing from the cases above.
      return exhaustiveCheck(e);
  }
}

function TurnView(props: { turn: Turn }) {
  // The cursor belongs on the last text block while the turn is still live.
  // We compute this per-event: an event gets streaming=true only when it is
  // a "text" block, the turn is not yet done, and it is the last event.
  const lastIdx = () => props.turn.events.length - 1;
  return (
    <div class="turn">
      <For each={props.turn.events}>{(event, i) => {
        const isStreamingText = () =>
          !props.turn.done &&
          event.type === "text" &&
          i() === lastIdx();
        return <EventBlock event={event} turnEvents={props.turn.events} streaming={isStreamingText()} />;
      }}</For>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sticky metrics bar (per-turn + session totals)
// ---------------------------------------------------------------------------

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms.toFixed(0)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function MetricsRow(props: {
  label: string;
  m: StickyMetrics;
  model?: string;
  provider?: string;
}) {
  const m = () => props.m;
  const line = () => {
    const p = props.provider;
    const isOpenAi = p === "openai";
    const costPrefix = isOpenAi ? "<=$" : "$";
    const parts: string[] = [
      `in: ${m().inputTokens}`,
      `out: ${m().outputTokens}`,
    ];
    if (!isOpenAi) {
      parts.push(`write: ${m().cacheCreationTokens}`);
      parts.push(`read: ${m().cacheReadTokens}`);
    }
    parts.push(`cost: ${costPrefix}${m().costUsd.toFixed(4)}`);
    if (!isOpenAi) {
      parts.push(`saved: ${m().savedUsd.toFixed(4)}`);
    }
    if (m().totalMs > 0) {
      parts.push(`dur: ${formatDuration(m().totalMs)}`);
    }
    if (props.model) {
      parts.push(`model: ${props.model}`);
    }
    return parts.join("  ");
  };
  return (
    <div class="sticky-metrics-row">
      <span class="sticky-metrics-label">{props.label}</span>
      <span class="sticky-metrics-body">{line()}</span>
    </div>
  );
}

function StickyMetricsBar() {
  return (
    <Show when={state.lastTurnEnd}>
      {(last) => (
        <div class="sticky-metrics">
          <MetricsRow
            label="turn:"
            m={last().metrics}
            model={last().model}
            provider={last().provider}
          />
          <MetricsRow
            label="session:"
            m={state.sessionTotals}
            provider={last().provider}
          />
        </div>
      )}
    </Show>
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

  // true  → auto-scroll to bottom on new content ("tailing")
  // false → user has scrolled up; show the ↓ button instead
  const [tailing, setTailing] = createSignal(true);

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

  // Detect manual scroll — switch to sticky mode when user scrolls up
  function onFeedScroll() {
    if (!feedRef) return;
    const atBottom = feedRef.scrollTop + feedRef.clientHeight >= feedRef.scrollHeight - 8;
    setTailing(atBottom);
  }

  // Scroll to bottom and return to tailing mode
  function scrollToBottom() {
    if (!feedRef) return;
    feedRef.scrollTop = feedRef.scrollHeight;
    setTailing(true);
  }

  // Auto-scroll to bottom on new content — only when tailing
  createEffect(() => {
    // Track turn count and the event list of the last turn (covers both new
    // turns and new events/text tokens appended to the current turn).
    const _ = state.turns.length;
    const lastTurn = state.turns[state.turns.length - 1];
    const __ = lastTurn?.events.length;
    if (tailing()) {
      queueMicrotask(() => {
        if (feedRef) feedRef.scrollTop = feedRef.scrollHeight;
      });
    }
  });

  return (
    <div class="app">
      <ActiveModal />
      <ReconnectBanner />
      <div class="feed-wrapper">
        <div class="feed" ref={feedRef} onScroll={onFeedScroll}>
          <ErrorBoundary fallback={(err) => (
            <div class="render-error">
              <strong>Render error</strong>
              <pre>{err?.message ?? String(err)}</pre>
            </div>
          )}>
            <For each={state.turns}>{(turn) => <TurnView turn={turn} />}</For>
          </ErrorBoundary>
        </div>
        <Show when={!tailing()}>
          <button class="scroll-to-bottom" onClick={scrollToBottom} title="Scroll to latest">
            ↓
          </button>
        </Show>
      </div>
      <StickyMetricsBar />
      <StatusDot />
      <InputArea />
    </div>
  );
}
