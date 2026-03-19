import { For, Show, ErrorBoundary, createEffect, onCleanup, createSignal, onMount, createMemo, createResource } from "solid-js";
import { state, dispatch, zeroMetrics, zeroDurations, type Turn, type WsEvent, type StickyMetrics, type DurationMetrics } from "./store";
import { marked } from "marked";

// Configure marked: GFM (tables, strikethrough), no raw HTML passthrough.
marked.setOptions({ gfm: true, breaks: false });
const _renderer = new marked.Renderer();
_renderer.html = ({ text }: { text: string }) =>
  text.replace(/</g, "&lt;").replace(/>/g, "&gt;");
marked.use({ renderer: _renderer });

/** Render markdown to an HTML string (raw HTML in source is escaped). */
function renderMarkdown(text: string): string {
  return marked.parse(text) as string;
}

/**
 * Format a duration in milliseconds for display.
 * < 1000ms → "NNNms", ≥ 1000ms → "N.Ns" (one decimal place).
 */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/**
 * Format a byte count as KB with context-appropriate precision.
 * perCall=true (payload modal): 2 decimal places, e.g. "14.23 KB"
 * perCall=false (turn/session bar): 1 decimal place, e.g. "67.8 KB"
 */
function formatKb(bytes: number, perCall = false): string {
  const kb = bytes / 1024;
  return perCall ? `${kb.toFixed(2)} KB` : `${kb.toFixed(1)} KB`;
}

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
  ts?: string;
  input: unknown;
  output: string;
  isError: boolean;
  durationMs: number;
}

interface LlmCallDetail {
  ts?: string;
  provider: string;
  url: string;
  model: string;
  contextHashes: string[];
  /** Number of hashes in the previous llm_call (0 if this is the first). */
  previousLength: number;
  requestBytes?: number;
  requestSummary?: Record<string, unknown>;
  /** Which event opened this modal — affects the title. */
  source?: "llm_call" | "llm_response";
}

interface LlmResponseDetail {
  ts?: string;
  streamingStart?: string;
  stopReason: string;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens?: number | null;
    cache_read_input_tokens?: number | null;
    service_tier?: string | null;
  };
  contextHash: string;
  /** Full hashes array: preceding llm_call's contextHashes + this response's contextHash. */
  allContextHashes: string[];
  text?: string;
  responseSummary?: Record<string, unknown>;
}

interface BlockDetail {
  label: string;
  ts?: string;
  streamingStart?: string;
  body: string;
}

type ModalContent =
  | { kind: "tool"; detail: ToolDetail }
  | { kind: "llm_call_messages"; detail: LlmCallDetail }
  | { kind: "llm_call_raw"; detail: LlmCallDetail }
  | { kind: "llm_response"; detail: LlmResponseDetail }
  | { kind: "block"; detail: BlockDetail };

const [activeModal, setActiveModal] = createSignal<ModalContent | null>(null);
const [legendOpen, setLegendOpen] = createSignal(false);

function setToolModal(d: ToolDetail | null) {
  setActiveModal(d ? { kind: "tool", detail: d } : null);
}

function closeModal() { setActiveModal(null); }

/** Render a context record's content array (or string) as a readable string. */
function renderContent(content: any): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content.map((block: any) => {
      if (block.type === "text") return block.text;
      if (block.type === "tool_use") return `[tool_use: ${block.name}]\n${JSON.stringify(block.input, null, 2)}`;
      if (block.type === "tool_result") {
        const out = Array.isArray(block.content)
          ? block.content.map((c: any) => c.text ?? JSON.stringify(c)).join("\n")
          : String(block.content ?? "");
        return `[tool_result]\n${out}`;
      }
      return JSON.stringify(block, null, 2);
    }).join("\n");
  }
  return JSON.stringify(content, null, 2);
}

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

// ---------------------------------------------------------------------------
// Token legend — fixed overlay, toggled by any ⓘ button in the UI
// ---------------------------------------------------------------------------

function TokenLegend() {
  const isOpenAi = () => {
    const p = state.liveTurn !== null ? state.liveProvider : (state.lastTurnEnd?.provider ?? "");
    return p === "openai";
  };
  return (
    <Show when={legendOpen()}>
      <div class="token-legend-overlay" onClick={() => setLegendOpen(false)}>
        <div class="token-legend" onClick={(e) => e.stopPropagation()}>
          <div class="token-legend-header">
            <span>Metrics fields</span>
            <button class="token-legend-close" onClick={() => setLegendOpen(false)}>✕</button>
          </div>
          <table class="token-legend-table">
            <tbody>
              <Show when={!isOpenAi()} fallback={<>
                <tr><td><code>in</code></td><td>Total prompt tokens sent</td></tr>
                <tr><td><code>out</code></td><td>Tokens generated by the model</td></tr>
              </>}>
                <tr><td><code>in (uncached)</code></td><td>Tokens processed fresh (not from cache)</td></tr>
                <tr><td><code>in (cache write)</code></td><td>Tokens written into the prompt cache (1.25×)</td></tr>
                <tr><td><code>in (cache read)</code></td><td>Tokens served from the prompt cache (0.1×)</td></tr>
                <tr><td><code>out</code></td><td>Tokens generated by the model</td></tr>
              </Show>
              <tr><td><code>request size</code></td><td>Size of the HTTP request body sent to the LLM</td></tr>
              <tr><td><code>llm</code></td><td>Time from HTTP request start to last byte of LLM response</td></tr>
              <tr><td><code>tools</code></td><td>Total time spent executing tool calls</td></tr>
              <tr><td><code>total</code></td><td>Total turn duration (LLM + tools + overhead)</td></tr>
            </tbody>
          </table>
        </div>
      </div>
    </Show>
  );
}

function ActiveModal() {
  return (
    <Show when={activeModal()}>
      {(m) => {
        const modal = m();
        if (modal.kind === "block") {
          const d = modal.detail;
          return (
            <ModalShell title={d.label} cls="block-modal">
              <Show when={d.streamingStart}>
                <div class="modal-section-label">streaming start: {formatTs(d.streamingStart)}</div>
              </Show>
              <Show when={d.ts}>
                <div class="modal-section-label">time: {formatTs(d.ts)}</div>
              </Show>
              <div class="modal-section-label">content</div>
              <pre class="modal-body">{d.body}</pre>
            </ModalShell>
          );
        }
        if (modal.kind === "tool") {
          const d = modal.detail;
          return (
            <ModalShell title={`tool › ${d.name}`} cls="tool-modal">
              <Show when={d.ts}>
                <div class="modal-section-label">{formatTs(d.ts)}</div>
              </Show>
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
        if (modal.kind === "llm_call_messages") {
          const d = modal.detail;
          const deltaCount = d.contextHashes.length - d.previousLength;

          // Fetch context records lazily when the modal opens.
          // hashes are ordered oldest→newest; we render newest→oldest.
          const [records] = createResource(
            () => d.contextHashes.join(","),
            async (hashParam) => {
              if (!hashParam) return [];
              const res = await fetch(`/context?hashes=${encodeURIComponent(hashParam)}`);
              if (!res.ok) return [];
              return res.json() as Promise<Array<{ hash: string; ts?: string; role: string; content: any }>>;
            },
          );

          return (
            <ModalShell title={`${d.source ?? "llm_call"} › messages`} cls="llm-call-modal">
              <Show when={d.ts}>
                <div class="modal-section-label">{formatTs(d.ts)}</div>
              </Show>
              <div class="modal-section-label">{d.contextHashes.length} messages · +{deltaCount} new</div>

              {/* Messages: newest first, separator between new and old */}
              <div class="llm-call-messages">
                <Show when={records.loading}>
                  <div class="llm-call-msg-loading">Loading…</div>
                </Show>
                <Show when={!records.loading}>
                  <For each={[...(records() ?? [])].reverse()}>
                    {(rec, i) => {
                      const reversedIdx = i(); // 0 = newest
                      const totalLen = (records() ?? []).length;
                      // In reversed order, index deltaCount-1 is the oldest new message.
                      // The separator goes AFTER it (between new and old).
                      const showSeparator = deltaCount > 0
                        && deltaCount < totalLen
                        && reversedIdx === deltaCount - 1;
                      return (
                        <>
                          <div class={`llm-call-msg llm-call-msg-${rec.role}`}>
                            <span class="llm-call-msg-role">{rec.role}<span class="llm-call-msg-ts">{rec.ts ? "  " + formatTs(rec.ts) : ""}</span></span>
                            <pre class="llm-call-msg-body">{renderContent(rec.content)}</pre>
                          </div>
                          <Show when={showSeparator}>
                            <div class="llm-call-separator">── already in context ──</div>
                          </Show>
                        </>
                      );
                    }}
                  </For>
                  <Show when={(records() ?? []).length === 0}>
                    <div class="llm-call-msg-loading">(context not available)</div>
                  </Show>
                </Show>
              </div>
            </ModalShell>
          );
        }
        if (modal.kind === "llm_call_raw") {
          const d = modal.detail;
          const deltaCount = d.contextHashes.length - d.previousLength;
          const rawStr = d.requestSummary != null
            ? JSON.stringify(d.requestSummary, null, 2)
            : "(request summary not available)";

          return (
            <ModalShell title="llm_call › payload" cls="llm-call-modal">
              <Show when={d.ts}>
                <div class="modal-section-label">{formatTs(d.ts)}</div>
              </Show>
              <div class="modal-section-label">{d.url}</div>
              <div class="modal-section-label">{d.contextHashes.length} messages · +{deltaCount} new</div>
              <Show when={d.requestBytes != null && d.requestBytes > 0}>
                <div class="modal-section-label">request size: {formatKb(d.requestBytes!, true)}</div>
              </Show>
              <pre class="modal-body">{rawStr}</pre>
            </ModalShell>
          );
        }
        // llm_response
        const d = modal.detail;
        const u = d.usage;
        const usageParts = [
          `in (uncached): ${u.input_tokens}`,
          ...(u.cache_creation_input_tokens ? [`in (cache write): ${u.cache_creation_input_tokens}`] : []),
          ...(u.cache_read_input_tokens     ? [`in (cache read): ${u.cache_read_input_tokens}`]      : []),
          `out: ${u.output_tokens}`,
          ...(u.service_tier && u.service_tier !== "standard" ? [`service_tier: ${u.service_tier}`] : []),
        ].join("  ");

        const respStr = d.responseSummary != null
          ? JSON.stringify(d.responseSummary, null, 2)
          : "(response summary not available)";

        const openMessages = () => setActiveModal({
          kind: "llm_call_messages",
          detail: {
            ts: d.ts,
            provider: "",
            url: "",
            model: "",
            contextHashes: d.allContextHashes,
            previousLength: d.allContextHashes.length - 1,
            requestSummary: undefined,
            source: "llm_response",
          },
        });

        return (
          <ModalShell title="llm_response › payload" cls="llm-resp-modal">
            <Show when={d.streamingStart}>
              <div class="modal-section-label">streaming start: {formatTs(d.streamingStart)}</div>
            </Show>
            <Show when={d.ts}>
              <div class="modal-section-label">time: {formatTs(d.ts)}</div>
            </Show>
            <div class="modal-section-label">{usageParts}  <button class="llm-legend-btn" onClick={() => setLegendOpen(o => !o)} title="Token legend">ⓘ</button>  <button class="llm-legend-btn" onClick={openMessages} title="View as messages">messages (+1)</button></div>
            <div class="modal-section-label">payload</div>
            <pre class="modal-body">{respStr}</pre>
          </ModalShell>
        );
      }}
    </Show>
  );
}

// ---------------------------------------------------------------------------
// Event block
// ---------------------------------------------------------------------------


function EventBlock(props: { event: WsEvent; turnEvents: WsEvent[]; allLlmCalls: Array<WsEvent & { type: "llm_call" }> }) {
  const e = props.event;
  const ts = (e as any).ts as string | undefined;
  const streamingStart = (e as any).streamingStart as string | undefined;

  // Exhaustive switch over WsEvent — every variant must have a case.
  // Compile-time guard: if a new WsEvent variant is added without a render
  // case, TypeScript will error on the exhaustiveCheck(e) call in default.
  switch (e.type) {
    case "user_message":
      return (
        <div class="block user">
          <div class="block-label-row">
            <span class="block-label">user_message</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "user_message", ts, body: e.content } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{e.content}</div>
        </div>
      );

    case "text":
      return (
        <div class="block assist">
          <div class="block-label-row">
            <span class="block-label">assistant_text</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "assistant_text", ts, streamingStart, body: e.text } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{e.text}</div>
        </div>
      );

    case "tool_call": {
      const inputPreview = createMemo(() => {
        if (e.input == null) return "(none)";
        const s = typeof e.input === "object" ? JSON.stringify(e.input) : String(e.input);
        return firstLine(s);
      });
      const shortId = (id: string) => id.length <= 6 ? id : `…${id.slice(-6)}`;
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
          ts,
          input: e.input,
          output: r ? r.output : "(not yet available)",
          isError: r ? r.isError : false,
          durationMs: r ? r.durationMs : 0,
        });
      };
      return (
        <div class="block tool">
          <div class="block-label-row">
            <span class="block-label">tool_call › {e.name}<span class="block-id"> [{shortId(e.id)}]</span></span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
          <div class="block-body block-preview">{inputPreview()}</div>
        </div>
      );
    }

    case "tool_result": {
      const outputPreview = createMemo(() => firstLine(e.output));
      const shortId = (id: string) => id.length <= 6 ? id : `…${id.slice(-6)}`;
      // Find matching tool_call for the modal
      const call = props.turnEvents.find(
        (ev): ev is WsEvent & { type: "tool_call" } =>
          ev.type === "tool_call" && (ev as any).id === e.id
      );
      const openModal = () => {
        setToolModal({
          name: e.name,
          ts,
          input: call ? call.input : null,
          output: e.output,
          isError: e.isError,
          durationMs: e.durationMs,
        });
      };
      return (
        <div class={`block result${e.isError ? " result-error" : ""}`}>
          <div class="block-label-row">
            <span class="block-label">tool_result › {e.name}<span class="block-id"> [{shortId(e.id)}]</span></span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
          <div class="block-body block-preview">{outputPreview()}</div>
        </div>
      );
    }

    case "model_changed": {
      const body = `Switched to ${e.provider} ${e.model}`;
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">model_changed</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "model_changed", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "oauth_token_expired": {
      const body = "OAuth token expired/revoked — refreshing…";
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">oauth_token_expired</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "oauth_token_expired", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "oauth_refreshed": {
      const body = "Token refreshed, retrying…";
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">oauth_refreshed</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "oauth_refreshed", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_user_start": {
      const body = "Compacting context…";
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">compact_user_start</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_user_start", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_user_done": {
      const body = e.messagesAfter === e.messagesBefore
        ? `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages (no change)`
        : `Context compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">compact_user_done</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_user_done", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_user_error": {
      const body = `⚠ Compaction failed: ${e.error}`;
      return (
        <div class="block error">
          <div class="block-label-row">
            <span class="block-label">compact_user_error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_user_error", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_auto_start": {
      const body = `Auto-compacting context (${e.messagesBefore} messages)…`;
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">compact_auto_start</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_auto_start", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_auto_done": {
      const body = `Context auto-compacted: ${e.messagesBefore} → ${e.messagesAfter} messages`;
      return (
        <div class="block status">
          <div class="block-label-row">
            <span class="block-label">compact_auto_done</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_auto_done", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compact_auto_error": {
      const body = `⚠ Auto-compaction failed (rolling truncation fallback): ${e.error}`;
      return (
        <div class="block error">
          <div class="block-label-row">
            <span class="block-label">compact_auto_error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compact_auto_error", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "world_state_saved": {
      const body = `✓ world state saved (${e.charCount} chars)`;
      return (
        <div class="block world-state-saved">
          <div class="block-label-row">
            <span class="block-label">world_state_saved</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "world_state_saved", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "llm_call": {
      // Find the previous llm_call across all turns to compute the delta.
      const myIdx = props.allLlmCalls.indexOf(e);
      const prevLlmCall = myIdx > 0 ? props.allLlmCalls[myIdx - 1] : undefined;
      const previousLength = prevLlmCall?.contextHashes.length ?? 0;
      const deltaCount = e.contextHashes.length - previousLength;

      const detail = {
        ts,
        provider: e.provider,
        url: e.url,
        model: e.model,
        contextHashes: e.contextHashes,
        previousLength,
        requestBytes: e.requestBytes,
        requestSummary: e.requestSummary,
      };
      const openMessages = () => setActiveModal({ kind: "llm_call_messages", detail });
      const openRaw      = () => setActiveModal({ kind: "llm_call_raw",      detail });
      return (
        <div class="block api-call">
          <div class="block-label-row">
            <span class="block-label">llm_call</span>
            <div class="block-btn-group">
              <button class="block-expand-btn" onClick={openMessages} title="View context messages">messages (+{deltaCount})</button>
              <button class="block-expand-btn" onClick={openRaw}      title="View API payload">payload</button>
            </div>
          </div>
        </div>
      );
    }

    case "llm_response": {
      // Find the preceding llm_call in this turn to get its contextHashes.
      const precedingCall = [...props.turnEvents]
        .reverse()
        .find((ev): ev is WsEvent & { type: "llm_call" } => ev.type === "llm_call");
      const allContextHashes = [
        ...(precedingCall?.contextHashes ?? []),
        e.contextHash,
      ];

      const openPayload = () => setActiveModal({
        kind: "llm_response",
        detail: {
          ts,
          streamingStart: e.streamingStart,
          stopReason: e.stopReason,
          usage: e.usage,
          contextHash: e.contextHash,
          allContextHashes,
          text: e.text,
          responseSummary: e.responseSummary,
        },
      });
      const openMessages = () => setActiveModal({
        kind: "llm_call_messages",
        detail: {
          ts,
          provider: "",
          url: "",
          model: "",
          contextHashes: allContextHashes,
          previousLength: allContextHashes.length - 1,
          requestSummary: undefined,
        },
      });
      return (
        <div class="block api-response">
          <div class="block-label-row">
            <span class="block-label">llm_response<span class="block-label-meta">{e.stopReason}</span></span>
            <div class="block-btn-group">
              <button class="block-expand-btn" onClick={openMessages} title="View as messages">messages (+1)</button>
              <button class="block-expand-btn" onClick={openPayload} title="View response payload">payload</button>
            </div>
          </div>
          <Show when={e.text}>
            <div class="block-body md-body" innerHTML={renderMarkdown(e.text!)} />
          </Show>
        </div>
      );
    }

    case "turn_end": {
      const m = e.metrics;
      const cacheDetail = (m.cacheCreationTokens || m.cacheReadTokens)
        ? `  in (cache write): ${m.cacheCreationTokens ?? 0}  in (cache read): ${m.cacheReadTokens ?? 0}`
        : "";
      const line = `in (uncached): ${m.inputTokens}${cacheDetail}  out: ${m.outputTokens}`;
      return (
        <div class="block footer">
          <div class="block-label-row">
            <span class="block-label">turn_end</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "turn_end", ts, body: line } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{line}  <button class="llm-legend-btn" onClick={() => setLegendOpen(o => !o)} title="Token legend">ⓘ</button></div>
        </div>
      );
    }

    case "llm_error": {
      const body = e.error;
      return (
        <div class="block error-b">
          <div class="block-label-row">
            <span class="block-label">api error ({e.provider})</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: `api error (${e.provider})`, ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "agent_error": {
      const body = e.error;
      return (
        <div class="block error-b">
          <div class="block-label-row">
            <span class="block-label">error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "agent_error", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "error": {
      const body = e.error;
      return (
        <div class="block error-b">
          <div class="block-label-row">
            <span class="block-label">error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "error", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "turn_interrupted":
      return (
        <div class="block interrupt">
          <div class="block-label-row">
            <span>⊘ Interrupted</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "turn_interrupted", ts, body: "⊘ Interrupted" } })} title="Details">⤢</button>
          </div>
        </div>
      );

    case "llm_retry": {
      const body = e.error;
      return (
        <div class="block info">
          <div class="block-label-row">
            <span class="block-label">llm retry (attempt {e.attempt})</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: `llm retry (attempt ${e.attempt})`, ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "session_start": {
      const body = `${e.authMode} · ${e.provider} · ${e.model}`;
      return (
        <div class="block info">
          <div class="block-label-row">
            <span class="block-label">session start</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "session_start", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "session_end": {
      const body = `${e.outcome}${e.reason ? ` — ${e.reason}` : ""}`;
      return (
        <div class="block info">
          <div class="block-label-row">
            <span class="block-label">session end</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "session_end", ts, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

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

function TurnView(props: { turn: Turn; allLlmCalls: Array<WsEvent & { type: "llm_call" }> }) {
  return (
    <div class="turn">
      <For each={props.turn.events}>{(event) => (
        <EventBlock event={event} turnEvents={props.turn.events} allLlmCalls={props.allLlmCalls} />
      )}</For>
      {/* Temporary streaming slot — visible only while text is arriving,
          before llm_response clears it (text is then on the llm_response event itself). */}
      <Show when={props.turn.streamingText}>
        <div class="block api-response streaming">
          <div class="block-label-row">
            <span class="block-label">llm_response</span>
          </div>
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

  // Duration for the turn row:
  //   - live turn → liveDurations (llmMs/toolMs tick up; turnMs=0 until turn_end)
  //   - completed turn → lastTurnEnd.durations (all three fields set)
  const turnDurations = (): DurationMetrics =>
    state.liveTurn !== null ? state.liveDurations : (state.lastTurnEnd?.durations ?? zeroDurations());

  const provider = (): string =>
    state.liveTurn !== null ? state.liveProvider : (state.lastTurnEnd?.provider ?? "");

  const isOpenAi = () => provider() === "openai";

  // Session row tokens: completed-turns total + live turn + compaction totals.
  const sessMetrics = (): StickyMetrics => {
    const base = state.sessionTotals;
    const live = state.liveTurn;
    const compact = state.compactionTotals;
    return {
      freshInTokens: base.freshInTokens + (live?.freshInTokens ?? 0) + compact.freshInTokens,
      writeInTokens: base.writeInTokens + (live?.writeInTokens ?? 0) + compact.writeInTokens,
      readInTokens:  base.readInTokens  + (live?.readInTokens  ?? 0) + compact.readInTokens,
      outTokens:     base.outTokens     + (live?.outTokens     ?? 0) + compact.outTokens,
      requestBytes:  base.requestBytes  + (live?.requestBytes  ?? 0) + compact.requestBytes,
    };
  };

  // Session row durations: completed-turns total + live turn durations.
  const sessDurations = (): DurationMetrics => {
    const base = state.sessionDurations;
    const live = state.liveDurations;
    return {
      llmMs:  base.llmMs  + live.llmMs,
      toolMs: base.toolMs + live.toolMs,
      turnMs: base.turnMs + live.turnMs,
    };
  };

  // ---------------------------------------------------------------------------
  // Column definitions — fixed schema, each column always present.
  // Value functions receive the metrics/durations for that row.
  // gap=true adds extra left padding to visually separate groups.
  // ---------------------------------------------------------------------------

  interface ColDef {
    label: string;
    gap: boolean;
    turnVal:    (m: StickyMetrics, d: DurationMetrics, isLive: boolean) => string;
    compactVal: (m: StickyMetrics) => string;
    sessVal:    (m: StickyMetrics, d: DurationMetrics) => string;
  }

  const tokenCols = (): ColDef[] => {
    if (isOpenAi()) {
      return [
        { label: "in",             gap: false, turnVal: (m) => String(m.freshInTokens), compactVal: (m) => String(m.freshInTokens), sessVal: (m) => String(m.freshInTokens) },
        { label: "out",            gap: true,  turnVal: (m) => String(m.outTokens),     compactVal: (m) => String(m.outTokens),     sessVal: (m) => String(m.outTokens) },
      ];
    }
    return [
      { label: "in (uncached)",    gap: false, turnVal: (m) => String(m.freshInTokens), compactVal: (m) => String(m.freshInTokens), sessVal: (m) => String(m.freshInTokens) },
      { label: "in (cache write)", gap: false, turnVal: (m) => String(m.writeInTokens), compactVal: (m) => String(m.writeInTokens), sessVal: (m) => String(m.writeInTokens) },
      { label: "in (cache read)",  gap: false, turnVal: (m) => String(m.readInTokens),  compactVal: (m) => String(m.readInTokens),  sessVal: (m) => String(m.readInTokens) },
      { label: "out",              gap: true,  turnVal: (m) => String(m.outTokens),     compactVal: (m) => String(m.outTokens),     sessVal: (m) => String(m.outTokens) },
    ];
  };

  // Fixed non-token columns always shown
  const fixedCols = (): ColDef[] => [
    {
      label: "request size",
      gap: true,
      turnVal:    (m) => formatKb(m.requestBytes),
      compactVal: (_m) => "—",
      sessVal:    (m) => formatKb(m.requestBytes),
    },
    {
      label: "llm",
      gap: true,
      turnVal:    (_m, d) => d.llmMs > 0 ? formatDuration(d.llmMs) : "—",
      compactVal: (_m) => "—",
      sessVal:    (_m, d) => d.llmMs > 0 ? formatDuration(d.llmMs) : "—",
    },
    {
      label: "tools",
      gap: false,
      turnVal:    (_m, d) => d.toolMs > 0 ? formatDuration(d.toolMs) : "—",
      compactVal: (_m) => "—",
      sessVal:    (_m, d) => d.toolMs > 0 ? formatDuration(d.toolMs) : "—",
    },
    {
      label: "total",
      gap: false,
      turnVal:    (_m, d, isLive) => (!isLive && d.turnMs > 0) ? formatDuration(d.turnMs) : "—",
      compactVal: (_m) => "—",
      sessVal:    (_m, _d) => "—",
    },
  ];

  const allCols = (): ColDef[] => [...tokenCols(), ...fixedCols()];

  const showCompact = () =>
    state.compactionTotals.outTokens > 0 ||
    state.compactionTotals.freshInTokens > 0 ||
    state.compactionTotals.writeInTokens > 0 ||
    state.compactionTotals.readInTokens > 0;

  return (
    <Show when={visible()}>
      <div class="sticky-metrics-wrap">
        <table class="sticky-metrics">
          <thead>
            <tr>
              <th class="sm-row-label sm-header-cell"></th>
              <For each={allCols()}>
                {(col) => (
                  <th class={`sm-header-cell${col.gap ? " sm-col-gap" : ""}`}>{col.label}</th>
                )}
              </For>
              <th class="sm-legend-cell sm-header-cell">
                <button class="sm-legend-toggle" onClick={() => setLegendOpen(o => !o)} title="Token legend">ⓘ</button>
              </th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td class="sm-row-label">turn</td>
              <For each={allCols()}>
                {(col) => (
                  <td class={`sm-col-val${col.gap ? " sm-col-gap" : ""}`}>
                    {col.turnVal(turnMetrics(), turnDurations(), state.liveTurn !== null)}
                  </td>
                )}
              </For>
              <td />
            </tr>
            <Show when={showCompact()}>
              <tr>
                <td class="sm-row-label">compact</td>
                <For each={allCols()}>
                  {(col) => (
                    <td class={`sm-col-val${col.gap ? " sm-col-gap" : ""}`}>
                      {col.compactVal(state.compactionTotals)}
                    </td>
                  )}
                </For>
                <td />
              </tr>
            </Show>
            <tr>
              <td class="sm-row-label">session</td>
              <For each={allCols()}>
                {(col) => (
                  <td class={`sm-col-val${col.gap ? " sm-col-gap" : ""}`}>
                    {col.sessVal(sessMetrics(), sessDurations())}
                  </td>
                )}
              </For>
              <td />
            </tr>
          </tbody>
        </table>
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
        />
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
      <TokenLegend />
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
            <For each={state.turns}>{(turn, turnIdx) => {
              // Collect all llm_call events across all turns up to and including
              // this one, in chronological order, for cross-turn delta computation.
              // Use turnIdx() (not indexOf) — indexOf on a SolidJS store proxy is
              // unreliable and can return -1, making every call look like the first.
              const allLlmCalls = createMemo(() =>
                state.turns
                  .slice(0, turnIdx() + 1)
                  .flatMap(t => t.events)
                  .filter((ev): ev is WsEvent & { type: "llm_call" } => ev.type === "llm_call")
              );
              return <TurnView turn={turn} allLlmCalls={allLlmCalls()} />;
            }}</For>
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
