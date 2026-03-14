import { For, Show, ErrorBoundary, createEffect, onCleanup, createSignal, onMount, createMemo } from "solid-js";
import { state, dispatch, zeroMetrics, type Turn, type WsEvent, type StickyMetrics } from "./store";

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
      const cacheDetail = (m.cacheCreationTokens || m.cacheReadTokens)
        ? `  write_in: ${m.cacheCreationTokens ?? 0}  read_in: ${m.cacheReadTokens ?? 0}`
        : "";
      const line = `in: ${m.inputTokens}${cacheDetail}  out: ${m.outputTokens}  model: ${e.model}`;
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
    case "session_info":
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
// Session bar — always-visible sticky line showing the current session dir
// ---------------------------------------------------------------------------

function SessionBar() {
  const activeProvider = (): string =>
    state.liveTurn !== null ? state.liveProvider : (state.lastTurnEnd?.provider ?? "");
  const activeModel = (): string =>
    state.liveTurn !== null ? state.liveModel : (state.lastTurnEnd?.model ?? "");

  return (
    <Show when={state.sessionDir}>
      <div class="session-bar">
        <span class="session-bar-label">session:</span>
        <span class="session-bar-dir">{state.sessionDir}</span>
        <Show when={activeProvider() || activeModel()}>
          <span class="session-bar-provider-model">
            {activeProvider()}{activeProvider() && activeModel() ? "\u2003" : ""}{activeModel()}
          </span>
        </Show>
      </div>
    </Show>
  );
}

// ---------------------------------------------------------------------------
// Sticky metrics bar (per-turn + session totals)
// ---------------------------------------------------------------------------


function StickyMetricsBar() {
  // Show if we have a live turn in progress OR a completed turn to display.
  const visible = () => state.liveTurn !== null || state.lastTurnEnd !== null;

  // During a live turn, use the live accumulator; otherwise fall back to lastTurnEnd.
  const turnMetrics = (): StickyMetrics =>
    state.liveTurn ?? state.lastTurnEnd?.metrics ?? zeroMetrics();

  const provider = (): string =>
    state.liveTurn !== null ? state.liveProvider : (state.lastTurnEnd?.provider ?? "");

  const isLive = () => state.liveTurn !== null;
  const isOpenAi = () => provider() === "openai";

  // Session row: completed-turns total + whatever is live right now.
  const sessMetrics = (): StickyMetrics => {
    const base = state.sessionTotals;
    const live = state.liveTurn;
    if (!live) return base;
    return {
      freshInTokens: base.freshInTokens + live.freshInTokens,
      writeInTokens: base.writeInTokens + live.writeInTokens,
      readInTokens:  base.readInTokens  + live.readInTokens,
      outTokens:     base.outTokens     + live.outTokens,
    };
  };

  // Build the [label, value, isGap] cell list for one row.
  // isGap=true marks the cell before `out` to render the visual separation.
  const buildCells = (m: StickyMetrics): Array<[string, string, boolean]> => {
    const cells: Array<[string, string, boolean]> = [];
    if (isOpenAi()) {
      cells.push(["in", String(m.freshInTokens), false]);
    } else {
      const totalIn = m.freshInTokens + m.writeInTokens + m.readInTokens;
      cells.push(["in (total)",       String(totalIn),          false]);
      cells.push(["in",               String(m.freshInTokens),  false]);
      cells.push(["in (cache write)", String(m.writeInTokens),  false]);
      cells.push(["in (cache read)",  String(m.readInTokens),   false]);
    }
    cells.push(["out", String(m.outTokens), true]);
    return cells;
  };

  const [legendOpen, setLegendOpen] = createSignal(false);

  const anthropicLegendBody = (
    <div class="sm-legend-body">
      <p><strong>Anthropic token fields</strong></p>
      <table class="sm-legend-table">
        <tbody>
          <tr><td><code>in (total)</code></td><td>All input tokens sent (= in + cache write + cache read)</td></tr>
          <tr><td><code>in</code></td><td>Tokens processed fresh (uncached)</td></tr>
          <tr><td><code>in (cache write)</code></td><td>Tokens written into the prompt cache</td></tr>
          <tr><td><code>in (cache read)</code></td><td>Tokens served from the prompt cache</td></tr>
          <tr><td><code>out</code></td><td>Tokens generated by the model</td></tr>
        </tbody>
      </table>
    </div>
  );

  const openAiLegendBody = (
    <div class="sm-legend-body">
      <p><strong>OpenAI token fields</strong></p>
      <table class="sm-legend-table">
        <tbody>
          <tr><td><code>in</code></td><td>Total prompt tokens</td></tr>
          <tr><td><code>out</code></td><td>Tokens generated by the model</td></tr>
        </tbody>
      </table>
    </div>
  );

  return (
    <Show when={visible()}>
      <div class="sticky-metrics-wrap">
        <table class="sticky-metrics">
          <tbody>
            <tr>
              <td class="sm-row-label">turn</td>
              <For each={buildCells(turnMetrics())}>
                {([lbl, val, gap]) => <>
                  <td class={gap ? "sm-col-label sm-col-gap" : "sm-col-label"}>{lbl}:</td>
                  <td class="sm-col-val">{val}</td>
                </>}
              </For>
            </tr>
            <tr>
              <td class="sm-row-label">session</td>
              <For each={buildCells(sessMetrics())}>
                {([lbl, val, gap]) => <>
                  <td class={gap ? "sm-col-label sm-col-gap" : "sm-col-label"}>{lbl}:</td>
                  <td class="sm-col-val">{val}</td>
                </>}
              </For>
              <td class="sm-legend-cell">
                <button class="sm-legend-toggle" onClick={() => setLegendOpen(o => !o)}>ⓘ</button>
              </td>
            </tr>
            <Show when={state.compactionTotals.outTokens > 0 || state.compactionTotals.freshInTokens > 0 || state.compactionTotals.writeInTokens > 0 || state.compactionTotals.readInTokens > 0}>
              <tr>
                <td class="sm-row-label">compact</td>
                <For each={buildCells(state.compactionTotals)}>
                  {([lbl, val, gap]) => <>
                    <td class={gap ? "sm-col-label sm-col-gap" : "sm-col-label"}>{lbl}:</td>
                    <td class="sm-col-val">{val}</td>
                  </>}
                </For>
              </tr>
            </Show>
          </tbody>
        </table>
        <Show when={legendOpen()}>
          {isOpenAi() ? openAiLegendBody : anthropicLegendBody}
        </Show>
      </div>
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
      <div class="textarea-wrap">
        <textarea
          ref={textareaRef}
          value={inputValue()}
          onInput={(e) => { setInputValue(e.currentTarget.value); autoResize(); }}
          onKeyDown={onKeyDown}
          placeholder="Message Omega… (Enter to send, Shift+Enter for newline)"
          rows={1}
          disabled={!state.connected}
          class={state.streaming ? "textarea-busy" : ""}
        />
        <Show when={state.streaming}>
          <div class="textarea-busy-notice">turn in progress — send disabled</div>
        </Show>
      </div>
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
        <div class="btn-group">
          <button class="send-btn" disabled title="Turn in progress — wait for it to finish or abort">
            Send
          </button>
          <button class="abort-btn" onClick={abort}>Abort</button>
        </div>
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
      <SessionBar />
      <StickyMetricsBar />
      <StatusDot />
      <InputArea />
    </div>
  );
}
