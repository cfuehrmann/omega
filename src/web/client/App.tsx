import { For, Show, ErrorBoundary, createEffect, onCleanup, createSignal, onMount, createMemo, createResource } from "solid-js";
import type { JSX } from "solid-js";
import { state, dispatch, setConnecting, handleDisconnect, zeroMetrics, zeroDurations, computeRenderGroups, type RenderGroup, type StickyMetrics, type DurationMetrics } from "./state";
import { ServerMessageSchema, type ServerMessage, type ClientMessage, type OmegaModel, type OmegaEffort } from "../protocol";
import { primaryToolArg } from "../../tools.schema.js";
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

/** Escape text for safe insertion into innerHTML. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Transform a diff/patch code block into line-coloured spans.
 * Each line becomes a `display: block` span so backgrounds span full width.
 * The `<pre>` receives the `diff-block` class, which removes its padding;
 * each span carries horizontal padding instead.
 */
function renderDiff(pre: HTMLPreElement, code: HTMLElement): void {
  const lines = (code.textContent ?? "").split("\n");
  // Drop the trailing empty entry from the final newline
  if (lines[lines.length - 1] === "") lines.pop();

  const html = lines.map(line => {
    const esc = escapeHtml(line);
    if (line.startsWith("+++") || line.startsWith("---")) {
      return `<span class="diff-file">${esc}</span>`;
    } else if (line.startsWith("+")) {
      return `<span class="diff-add">${esc}</span>`;
    } else if (line.startsWith("-")) {
      return `<span class="diff-del">${esc}</span>`;
    } else if (line.startsWith("@@")) {
      return `<span class="diff-hunk">${esc}</span>`;
    } else {
      return `<span class="diff-ctx">${esc}</span>`;
    }
  }).join(""); // display:block handles line breaks; no separator needed

  code.innerHTML = html;
  pre.classList.add("diff-block");
  pre.setAttribute("data-testid", "diff-block");
}

/** Inject a copy button into an element that copies the given text on click. */
function addCopyButton(pre: HTMLElement, textToCopy: string): void {
  const btn = document.createElement("button");
  btn.className = "code-copy-btn";
  btn.setAttribute("data-testid", "code-copy-btn");
  btn.setAttribute("aria-label", "copy code");
  btn.textContent = "copy";
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    navigator.clipboard.writeText(textToCopy).then(() => {
      btn.textContent = "✓";
      setTimeout(() => { btn.textContent = "copy"; }, 1500);
    }).catch(() => {
      btn.textContent = "err";
      setTimeout(() => { btn.textContent = "copy"; }, 1500);
    });
  });
  pre.appendChild(btn);
}

/**
 * Post-process all `<pre>` blocks inside a rendered markdown container:
 * add copy buttons and apply diff colouring where applicable.
 * Mermaid blocks are marked `.mermaid-pending` for async rendering.
 * Idempotent — skips blocks already marked with `data-enhanced`.
 */
function enhanceCodeBlocks(container: HTMLElement): void {
  container.querySelectorAll<HTMLPreElement>("pre").forEach(pre => {
    if (pre.dataset.enhanced) return;
    pre.dataset.enhanced = "1";

    const code = pre.querySelector("code");
    // Capture raw text before any DOM transformation
    const textToCopy = code?.textContent ?? pre.textContent ?? "";

    if (code?.className.includes("language-mermaid")) {
      // Copy button and SVG will be added to a wrapper by renderMermaidBlocks.
      // Store the source so the wrapper can copy it.
      pre.dataset.mermaidSource = textToCopy;
      pre.classList.add("mermaid-pending");
      return;
    }

    addCopyButton(pre, textToCopy);

    if (code) {
      const cls = code.className;
      if (cls.includes("language-diff") || cls.includes("language-patch")) {
        renderDiff(pre, code);
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Mermaid — lazy-loaded, rendered async after markdown settles
// ---------------------------------------------------------------------------

let _mermaid: typeof import("mermaid").default | null = null;
let _mermaidInitialised = false;
let _mermaidCounter = 0;

async function getMermaid(): Promise<typeof import("mermaid").default> {
  if (!_mermaid) {
    const mod = await import("mermaid");
    _mermaid = mod.default;
  }
  if (!_mermaidInitialised) {
    _mermaid.initialize({ startOnLoad: false, theme: "dark" });
    _mermaidInitialised = true;
  }
  return _mermaid;
}

/**
 * Find all `.mermaid-pending` `<pre>` elements inside `container`, render
 * each as an SVG diagram, and replace them with a wrapper div that carries
 * the diagram (or an error notice + raw source on failure) and a copy button.
 */
async function renderMermaidBlocks(container: HTMLElement): Promise<void> {
  const blocks = Array.from(
    container.querySelectorAll<HTMLPreElement>("pre.mermaid-pending"),
  );
  if (blocks.length === 0) return;

  // Remove class synchronously before any await so concurrent calls
  // cannot pick up the same elements.
  blocks.forEach(pre => pre.classList.remove("mermaid-pending"));

  const mermaid = await getMermaid();

  for (const pre of blocks) {
    const source = pre.dataset.mermaidSource ?? pre.textContent ?? "";
    const id = `mermaid-svg-${++_mermaidCounter}`;

    const wrapper = document.createElement("div");
    wrapper.className = "mermaid-wrapper";
    wrapper.setAttribute("data-testid", "mermaid-wrapper");

    // Copy button on the wrapper (copies the raw source text)
    addCopyButton(wrapper, source);

    try {
      const { svg } = await mermaid.render(id, source);
      const diagram = document.createElement("div");
      diagram.className = "mermaid-diagram";
      diagram.setAttribute("data-testid", "mermaid-diagram");
      diagram.innerHTML = svg;
      wrapper.appendChild(diagram);
    } catch (err) {
      wrapper.classList.add("mermaid-error");
      const notice = document.createElement("div");
      notice.className = "mermaid-error-notice";
      notice.setAttribute("data-testid", "mermaid-error-notice");
      notice.textContent = `⚠ Mermaid error: ${err instanceof Error ? err.message : String(err)}`;
      wrapper.appendChild(notice);
      // Show the raw source so the user can read/fix it
      const sourcePre = document.createElement("pre");
      sourcePre.className = "mermaid-source";
      sourcePre.setAttribute("data-testid", "mermaid-source");
      sourcePre.textContent = source;
      wrapper.appendChild(sourcePre);
    }

    pre.replaceWith(wrapper);
  }
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

/** Compile-time exhaustiveness guard for ServerMessage switch in EventBlock. */
function exhaustiveCheck(x: never): null {
  console.warn("Unhandled ServerMessage type:", (x as any).type);
  return null;
}

/**
 * Format an ISO timestamp string to local date + time with millisecond
 * precision: "YYYY-MM-DD HH:mm:ss.mmm". Returns "" when time is absent.
 */
function formatTs(time: string | undefined): string {
  if (!time) return "";
  const d = new Date(time);
  if (isNaN(d.getTime())) return time; // fallback: show raw string
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
    setConnecting();
  };

  ws.onerror = () => {
    // onclose fires right after onerror — let onclose handle the state update
  };

  ws.onmessage = (e) => {
    let event: ServerMessage;
    try { event = ServerMessageSchema.parse(JSON.parse(e.data as string)); } catch { return; }
    dispatch(event);

    // Session picker integration: refresh on delete
    if (event.type === "session_deleted") {
      _onSessionDeleted(event.sessionDir);
    }
    if (event.type === "session_renamed") {
      _onSessionRenamed(event.sessionDir, event.name);
    }
  };

  ws.onclose = () => {
    handleDisconnect();
    reconnectTimer = setTimeout(connect, 2000);
  };
}

function sendToServer(msg: ClientMessage) {
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}

// ---------------------------------------------------------------------------
// MdBody — renders settled markdown with copy buttons and diff colouring
// ---------------------------------------------------------------------------

/**
 * Renders markdown text to HTML and post-processes code blocks.
 * Uses createEffect so SolidJS tracks `props.text` as a reactive dependency;
 * DOM updates happen in the same microtask frame as the signal change.
 */
function MdBody(props: { text: string }) {
  let ref!: HTMLDivElement;
  createEffect(() => {
    ref.innerHTML = renderMarkdown(props.text);
    enhanceCodeBlocks(ref);
    void renderMermaidBlocks(ref);
  });
  return <div class="block-body md-body" data-testid="md-body" ref={ref} />;
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

// primaryToolArg is imported from tools.schema.ts — single source of
// tool-name-to-display-arg mapping shared with the terminal formatter.

/** Sequential number for a tool call within one LLM response batch (1-based).
 *  Returns null when the batch has only one call — no number needed. */
function toolSeq(turnEvents: ServerMessage[], id: string, contextHash: string): number | null {
  const calls = turnEvents.filter(ev =>
    ev.type === "tool_call" &&
    (ev as { contextHash?: string }).contextHash === contextHash
  );
  if (calls.length <= 1) return null;
  const idx = calls.findIndex(ev => (ev as { id?: string }).id === id);
  return idx + 1;
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

interface ToolDetail {
  name: string;
  time?: string;
  input: unknown;
  output: string;
  isError: boolean;
  durationMs: number;
}

interface LlmCallDetail {
  time?: string;
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
  time?: string;
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
  clearedToolUses?: number;
  clearedInputTokens?: number;
}

interface BlockDetail {
  label: string;
  time?: string;
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
const [panelOpen, setPanelOpen] = createSignal(false);

function setToolModal(d: ToolDetail | null) {
  setActiveModal(d ? { kind: "tool", detail: d } : null);
}

function closeModal() { setActiveModal(null); }

/** Render a context record's content array (or string) as a readable string. */
function renderContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content.map((block) => {
      const b = block as Record<string, unknown>;
      if (b.type === "text") return String(b.text ?? "");
      if (b.type === "tool_use") return `[tool_use: ${String(b.name ?? "")}]\n${JSON.stringify(b.input, null, 2)}`;
      if (b.type === "tool_result") {
        const out = Array.isArray(b.content)
          ? b.content.map((c) => {
              const cr = c as Record<string, unknown>;
              return cr.text != null ? String(cr.text) : JSON.stringify(c);
            }).join("\n")
          : String(b.content ?? "");
        return `[tool_result]\n${out}`;
      }
      return JSON.stringify(block, null, 2);
    }).join("\n");
  }
  return JSON.stringify(content, null, 2);
}

function ModalShell(props: { title: string; cls: string; testId?: string; children: JSX.Element }) {
  return (
    <div class="modal-backdrop">
      <div class={`modal ${props.cls}`} data-testid={props.testId}>
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
              <tr><td><code>in (uncached)</code></td><td>Tokens processed fresh (not from cache)</td></tr>
              <tr><td><code>in (cache write)</code></td><td>Tokens written into the prompt cache (1.25×)</td></tr>
              <tr><td><code>in (cache read)</code></td><td>Tokens served from the prompt cache (0.1×)</td></tr>
              <tr><td><code>out</code></td><td>Tokens generated by the model</td></tr>
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
              <Show when={d.time}>
                <div class="modal-section-label">time: {formatTs(d.time)}</div>
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
              <div class="modal-scroll-body">
                <Show when={d.time}>
                  <div class="modal-section-label">{formatTs(d.time)}</div>
                </Show>
                <div class="modal-section-label">input</div>
                <pre class="modal-pre">{
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
                <pre class="modal-pre">{d.output}</pre>
              </div>
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
              return res.json() as Promise<Array<{ hash: string; time?: string; role: string; content: unknown }>>;
            },
          );

          return (
            <ModalShell title={`${d.source ?? "llm_call"} › messages`} cls="llm-call-modal" testId="llm-call-modal">
              <Show when={d.time}>
                <div class="modal-section-label">{formatTs(d.time)}</div>
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
                            <span class="llm-call-msg-role" data-testid="llm-call-msg-role">{rec.role}<span class="llm-call-msg-ts">{rec.time ? "  " + formatTs(rec.time) : ""}</span></span>
                            <pre class="llm-call-msg-body" data-testid="llm-call-msg-body">{renderContent(rec.content)}</pre>
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
              <Show when={d.time}>
                <div class="modal-section-label">{formatTs(d.time)}</div>
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
            time: d.time,
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
            <Show when={d.time}>
              <div class="modal-section-label">time: {formatTs(d.time)}</div>
            </Show>
            <Show when={d.clearedToolUses}>
              <div class="modal-section-label">tools cleared: {d.clearedToolUses} · tokens saved: ~{d.clearedInputTokens?.toLocaleString()}</div>
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


function EventBlock(props: { event: ServerMessage; turnEvents: ServerMessage[]; allLlmCalls: Array<ServerMessage & { type: "llm_call" }> }) {
  const e = props.event;
  const time = "time" in e ? (e as { time?: string }).time : undefined;
  const streamingStart = "streamingStart" in e ? (e as { streamingStart?: string }).streamingStart : undefined;

  // Exhaustive switch over ServerMessage — every variant must have a case.
  // Compile-time guard: if a new ServerMessage variant is added without a render
  // case, TypeScript will error on the exhaustiveCheck(e) call in default.
  switch (e.type) {
    case "user_message":
      return (
        <div class="block user" data-testid="block-user">
          <div class="block-label-row">
            <span class="block-label">user_message</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "user_message", time, body: e.content } })} title="Details">⤢</button>
          </div>
          <div class="user-msg-text">{e.content}</div>
        </div>
      );

    case "text":
      return (
        <div class="block assist" data-testid="block-llm-response">
          <div class="block-label-row">
            <span class="block-label">assistant_text</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "assistant_text", time, streamingStart, body: e.text } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{e.text}</div>
        </div>
      );

    case "tool_call": {
      const arg = createMemo(() => primaryToolArg(e.name, e.input));
      const seq = createMemo(() => toolSeq(props.turnEvents, e.id, e.contextHash));
      // Find the matching tool_result in the same turn by id
      const result = createMemo(() =>
        props.turnEvents.find(
          (ev): ev is ServerMessage & { type: "tool_result" } =>
            ev.type === "tool_result" && ev.id === e.id
        )
      );
      const openModal = () => {
        const r = result();
        setToolModal({
          name: e.name,
          time,
          input: e.input,
          output: r ? r.output : "(not yet available)",
          isError: r ? r.isError : false,
          durationMs: r ? r.durationMs : 0,
        });
      };
      return (
        <div class="block tool" data-testid="block-tool">
          <div class="block-label-row block-tool-row">
            <span class="tool-call-content">
              <Show when={seq() != null}><span class="tool-seq">{seq()}</span></Show>
              <span class="tool-name">{e.name}</span>
              <span class="tool-arg">{arg()}</span>
            </span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
        </div>
      );
    }

    case "tool_result": {
      // Find matching tool_call for the modal and sequence number
      const call = createMemo(() =>
        props.turnEvents.find(
          (ev): ev is ServerMessage & { type: "tool_call" } =>
            ev.type === "tool_call" && ev.id === e.id
        )
      );
      const seq = createMemo(() => {
        const c = call();
        return c ? toolSeq(props.turnEvents, e.id, c.contextHash) : null;
      });
      const openModal = () => {
        const c = call();
        setToolModal({
          name: e.name,
          time,
          input: c ? c.input : null,
          output: e.output,
          isError: e.isError,
          durationMs: e.durationMs,
        });
      };
      return (
        <div class={`block result${e.isError ? " result-error" : ""}`} data-testid="block-result" data-error={e.isError ? "true" : undefined}>
          <div class="block-label-row">
            <span class="tool-result-left">
              <Show when={seq() != null}><span class="tool-seq">{seq()}</span></Show>
              <span class="block-label">tool_result</span>
            </span>
            <button class="block-expand-btn" onClick={openModal} title="View full input/output">⤢</button>
          </div>
          <div class="block-body block-preview-result">{e.output}</div>
        </div>
      );
    }

    case "model_changed": {
      const body = `Switched to ${e.model}`;
      return (
        <div class="block status" data-testid="block-status">
          <div class="block-label-row">
            <span class="block-label">model_changed</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "model_changed", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "effort_changed": {
      const body = `Effort set to ${e.effort}`;
      return (
        <div class="block status" data-testid="block-status">
          <div class="block-label-row">
            <span class="block-label">effort_changed</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "effort_changed", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "compacted": {
      const body = "Context compacted by server.";
      return (
        <div class="block status" data-testid="block-status">
          <div class="block-label-row">
            <span class="block-label">compacted</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "compacted", time, body: JSON.stringify(e.usage, null, 2) } })} title="Details">⤢</button>
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
        time,
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
        <div class="block api-call" data-testid="block-llm-call">
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
      // Find the preceding llm_call in this group to get its contextHashes.
      const precedingCall = [...props.turnEvents]
        .reverse()
        .find((ev): ev is ServerMessage & { type: "llm_call" } => ev.type === "llm_call");
      const allContextHashes = [
        ...(precedingCall?.contextHashes ?? []),
        e.contextHash,
      ];

      const openPayload = () => setActiveModal({
        kind: "llm_response",
        detail: {
          time,
          streamingStart: e.streamingStart,
          stopReason: e.stopReason,
          usage: e.usage,
          contextHash: e.contextHash,
          allContextHashes,
          text: e.text,
          responseSummary: e.responseSummary,
          clearedToolUses: e.clearedToolUses,
          clearedInputTokens: e.clearedInputTokens,
        },
      });
      const openMessages = () => setActiveModal({
        kind: "llm_call_messages",
        detail: {
          time,
          url: "",
          model: "",
          contextHashes: allContextHashes,
          previousLength: allContextHashes.length - 1,
          requestSummary: undefined,
        },
      });
      const openThinking = () => setActiveModal({
        kind: "block",
        detail: {
          label: "thinking",
          time,
          body: e.thinking ?? "",
        },
      });
      return (
        <div class="block api-response" data-testid="block-llm-response">
          <div class="block-label-row">
            <span class="block-label">llm_response<span class="block-label-meta">{e.stopReason}</span><Show when={e.clearedToolUses}><span class="block-label-meta"> · {e.clearedToolUses} tools cleared</span></Show></span>
            <div class="block-btn-group">
              <Show when={e.thinking}>
                <button class="block-expand-btn thinking-btn" onClick={openThinking} title="View thinking">thinking</button>
              </Show>
              <button class="block-expand-btn" onClick={openMessages} title="View as messages">messages (+1)</button>
              <button class="block-expand-btn" onClick={openPayload} title="View response payload">payload</button>
            </div>
          </div>
          <Show when={e.text}>
            <MdBody text={e.text!} />
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
        <div class="block footer" data-testid="block-turn-end">
          <div class="block-label-row">
            <span class="block-label">turn_end</span>
            <span class="turn-end-line">{line}  <button class="llm-legend-btn" onClick={() => setLegendOpen(o => !o)} title="Token legend">ⓘ</button></span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "turn_end", time, body: line } })} title="Details">⤢</button>
          </div>
        </div>
      );
    }

    case "llm_error": {
      const body = e.error;
      return (
        <div class="block error-b" data-testid="block-error">
          <div class="block-label-row">
            <span class="block-label">llm_error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "llm_error", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "agent_error": {
      const body = e.error;
      return (
        <div class="block error-b" data-testid="block-error">
          <div class="block-label-row">
            <span class="block-label">agent_error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "agent_error", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "transport_error": {
      const body = e.error;
      return (
        <div class="block error-b" data-testid="block-error">
          <div class="block-label-row">
            <span class="block-label">transport_error</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "transport_error", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "turn_interrupted": {
      const reason = e.reason;
      const label =
        reason === "aborted" ? "⊘ Aborted"
        : reason === "error"   ? "⊘ Failed"
        :                        "⊘ Interrupted";
      return (
        <div class="block interrupt" data-testid="block-interrupt">
          <div class="block-label-row">
            <span>{label}</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "turn_interrupted", time, body: label + (reason ? ` (reason: ${reason})` : "") } })} title="Details">⤢</button>
          </div>
        </div>
      );
    }

    case "llm_retry": {
      const statusStr = e.httpStatus ? `HTTP ${e.httpStatus}` : "network error";
      const waitSec = (e.waitMs / 1000).toFixed(1);
      const retryAtStr = e.retryAt ? formatTs(e.retryAt) : undefined;
      const headline = `${statusStr} · retrying in ${waitSec}s${retryAtStr ? ` → ${retryAtStr}` : ""}`;
      const bodyFull = e.errorBody != null
        ? `${e.error}\n\n${JSON.stringify(e.errorBody, null, 2)}`
        : e.error;
      return (
        <div class="block retry" data-testid="block-retry">
          <div class="block-label-row">
            <span class="block-label">⟳ retry · attempt {e.attempt}</span>
            <div class="block-btn-group">
              <span class="block-retry-meta">{headline}</span>
              <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: `retry · attempt ${e.attempt}`, time, body: bodyFull } })} title="Full error details">⤢</button>
            </div>
          </div>
          <div class="block-body">{e.error}</div>
          <Show when={e.thinkingFragment}>
            <div class="retry-fragment">
              <span class="retry-fragment-label">thinking before interruption</span>
              <pre class="retry-fragment-body">{e.thinkingFragment}</pre>
            </div>
          </Show>
          <Show when={e.textFragment}>
            <div class="retry-fragment">
              <span class="retry-fragment-label">text before interruption</span>
              <pre class="retry-fragment-body">{e.textFragment}</pre>
            </div>
          </Show>
        </div>
      );
    }

    case "session_started": {
      const body = `${e.path} · ${e.model} · ${(e as any).effort ?? "medium"}`;
      return (
        <div class="block info" data-testid="block-info">
          <div class="block-label-row">
            <span class="block-label">session_started</span>
            <span class="user-msg-body">{body}</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "session_started", time, body } })} title="Details">⤢</button>
          </div>
        </div>
      );
    }

    case "server_started":
      return (
        <div class="block info" data-testid="block-info">
          <div class="block-label-row">
            <span class="block-label">server_started</span>
          </div>
        </div>
      );

    case "server_stopped": {
      const body = `${e.outcome}${e.reason ? ` — ${e.reason}` : ""}`;
      return (
        <div class="block info" data-testid="block-info">
          <div class="block-label-row">
            <span class="block-label">server_stopped</span>
            <button class="block-expand-btn" onClick={() => setActiveModal({ kind: "block", detail: { label: "server_stopped", time, body } })} title="Details">⤢</button>
          </div>
          <div class="block-body">{body}</div>
        </div>
      );
    }

    case "resuming_session": {
      const label = e.name
        ? `↩ resuming "${e.name}" (${e.resumedFrom})`
        : `↩ resuming from ${e.resumedFrom}`;
      const openBasis = () => setActiveModal({
        kind: "block",
        detail: { label: "resuming_session · basis", time, body: e.basis },
      });
      return (
        <div class="block info" data-testid="block-resuming-session">
          <div class="block-label-row">
            <span class="block-label">resuming_session</span>
            <span class="block-body resumed-label">{label}</span>
            <div class="block-btn-group">
              <button class="block-expand-btn" onClick={openBasis} title="View basis">basis</button>
            </div>
          </div>
        </div>
      );
    }

    case "session_resumed": {
      const label = `↩ resumed from ${e.resumedFrom}`;
      const openSummary = () => setActiveModal({
        kind: "block",
        detail: { label: "session_resumed · summary", time, body: e.summary },
      });
      return (
        <div class="block info" data-testid="block-session-resumed">
          <div class="block-label-row">
            <span class="block-label">session_resumed</span>
            <span class="block-body resumed-label">{label}</span>
            <div class="block-btn-group">
              <button class="block-expand-btn" onClick={openSummary} title="View summary">summary</button>
            </div>
          </div>
        </div>
      );
    }

    // Protocol envelope events — handled by dispatch(), never appear in turn.events.
    // Listed here to satisfy the exhaustive check.
    case "ready":
    case "history":
    case "reset_done":
    case "session_info":
    case "session_deleted":
    case "session_renamed":
    // thinking is a streaming-only signal — never pushed into turn.events
    case "thinking":
      return null;

    default:
      // Compile-time exhaustiveness guard: TypeScript errors here if any
      // ServerMessage variant is missing from the cases above.
      return exhaustiveCheck(e);
  }
}

function GroupView(props: {
  group: RenderGroup;
  isLast: boolean;
  allLlmCalls: Array<ServerMessage & { type: "llm_call" }>;
}) {
  return (
    <>
      <For each={props.group.events}>{(event) => (
        <EventBlock event={event} turnEvents={props.group.events} allLlmCalls={props.allLlmCalls} />
      )}</For>
      {/* Streaming slots: only shown for the last group (the one currently receiving
          or that most recently received content). No done-guard — if llm_response
          never arrives before turn_end, any accumulated streaming text stays
          visible (matching old behaviour). The text is cleared on the next
          user_message so it never bleeds into a following turn. */}
      <Show when={props.isLast && state.streamingThinking}>
        <div class="block thinking streaming" data-testid="block-thinking">
          <div class="block-label-row">
            <span class="block-label">thinking</span>
          </div>
          <div class="block-body">
            <pre class="thinking-body">{state.streamingThinking}</pre>
            <span class="cursor" />
          </div>
        </div>
      </Show>
      <Show when={props.isLast && state.streamingText}>
        <div class="block api-response streaming" data-testid="block-llm-response">
          <div class="block-label-row">
            <span class="block-label">llm_response</span>
          </div>
          <div class="block-body">
            {state.streamingText}
            <span class="cursor" />
          </div>
        </div>
      </Show>
    </>
  );
}

// ---------------------------------------------------------------------------
// Session bar — always-visible sticky line showing the current session dir
// and model selector.
// ---------------------------------------------------------------------------

// Module-level helpers used by BottomPanel and InputRow
function activeModel(): string {
  return state.liveTurn !== null ? state.liveModel : (state.lastTurnEnd?.model ?? state.liveModel);
}

function handleModelChange(model: OmegaModel) {
  sendToServer({ type: "set_model", model });
}

function activeEffort(): string {
  return state.liveEffort;
}



function newSession() {
  if (!state.connected || state.streaming) return;
  sendToServer({ type: "reset" });
}

// ---------------------------------------------------------------------------
// Sticky metrics bar (per-turn + session totals)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// MetricsTable — renders the token/duration table, used inside BottomPanel
// ---------------------------------------------------------------------------

function MetricsTable() {
  // Ticking wall-clock signal — updates every second while a turn is live.
  const [now, setNow] = createSignal(Date.now());
  createEffect(() => {
    if (!state.streaming) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(id));
  });

  // Timestamp of the current turn's user_message — null when not streaming.
  const liveTurnStartMs = createMemo(() => {
    if (!state.streaming) return null;
    const events = state.events;
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i]!;
      if (e.type === "user_message" && e.time) return new Date(e.time).getTime();
    }
    return null;
  });

  // Elapsed ms for the current live turn (0 when idle).
  const liveTurnElapsedMs = (): number => {
    const start = liveTurnStartMs();
    return start !== null ? Math.max(0, now() - start) : 0;
  };

  const turnMetrics = (): StickyMetrics =>
    state.liveTurn ?? state.lastTurnEnd?.metrics ?? zeroMetrics();

  const turnDurations = (): DurationMetrics =>
    state.liveTurn !== null ? state.liveDurations : (state.lastTurnEnd?.durations ?? zeroDurations());

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

  const sessDurations = (): DurationMetrics => {
    const base = state.sessionDurations;
    const live = state.liveDurations;
    return {
      llmMs:  base.llmMs  + live.llmMs,
      toolMs: base.toolMs + live.toolMs,
      turnMs: base.turnMs + live.turnMs,
    };
  };

  interface ColDef {
    label: string;
    gap: boolean;
    turnVal:    (m: StickyMetrics, d: DurationMetrics, isLive: boolean) => string;
    sessVal:    (m: StickyMetrics, d: DurationMetrics) => string;
  }

  const allCols = (): ColDef[] => [
    { label: "in (uncached)",    gap: false, turnVal: (m) => String(m.freshInTokens), sessVal: (m) => String(m.freshInTokens) },
    { label: "in (cache write)", gap: false, turnVal: (m) => String(m.writeInTokens), sessVal: (m) => String(m.writeInTokens) },
    { label: "in (cache read)",  gap: false, turnVal: (m) => String(m.readInTokens),  sessVal: (m) => String(m.readInTokens) },
    { label: "out",              gap: true,  turnVal: (m) => String(m.outTokens),     sessVal: (m) => String(m.outTokens) },
    { label: "request size",     gap: true,  turnVal: (m) => formatKb(m.requestBytes), sessVal: (m) => formatKb(m.requestBytes) },
    { label: "llm",              gap: true,  turnVal: (_m, d) => d.llmMs > 0 ? formatDuration(d.llmMs) : "—",  sessVal: (_m, d) => d.llmMs > 0 ? formatDuration(d.llmMs) : "—" },
    { label: "tools",            gap: false, turnVal: (_m, d) => d.toolMs > 0 ? formatDuration(d.toolMs) : "—", sessVal: (_m, d) => d.toolMs > 0 ? formatDuration(d.toolMs) : "—" },
    { label: "total",            gap: false, turnVal: (_m, d, isLive) => isLive ? formatDuration(liveTurnElapsedMs()) : (d.turnMs > 0 ? formatDuration(d.turnMs) : "—"), sessVal: (_m, d) => { const t = d.turnMs + (state.streaming ? liveTurnElapsedMs() : 0); return t > 0 ? formatDuration(t) : "—"; } },
  ];

  const showCompact = () =>
    state.compactionTotals.outTokens > 0 ||
    state.compactionTotals.freshInTokens > 0 ||
    state.compactionTotals.writeInTokens > 0 ||
    state.compactionTotals.readInTokens > 0;

  return (
    <table class="metrics-table">
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
            <td colspan={allCols().length} class="sm-compact-line">
              in: {state.compactionTotals.freshInTokens.toLocaleString()}
              {"\u2003"}
              out: {state.compactionTotals.outTokens.toLocaleString()}
            </td>
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
  );
}

// ---------------------------------------------------------------------------
// BottomPanel — collapsible panel (session info + metrics), toggled by Ω
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Session picker modal
// ---------------------------------------------------------------------------

interface SessionItem {
  dir: string;
  name?: string;
  description?: string;
  resumedFrom?: string;
  lastActivity: string;
}

const [sessionPickerOpen, setSessionPickerOpen] = createSignal(false);

/**
 * True when the client is connected but no session has been created or resumed
 * yet. In this state the session picker is shown automatically and cannot be
 * dismissed — the user must choose before any work can begin.
 */
const needsSessionChoice = () => state.connected && !state.sessionDir;

// Track deleted session dirs so the picker can filter them out client-side
const [deletedSessions, setDeletedSessions] = createSignal<Set<string>>(new Set());
function _onSessionDeleted(dir: string) {
  setDeletedSessions(prev => { const next = new Set(prev); next.add(dir); return next; });
}
// Track renamed sessions so the picker can update names client-side
const [renamedSessions, setRenamedSessions] = createSignal<Map<string, string>>(new Map());
function _onSessionRenamed(dir: string, name: string) {
  setRenamedSessions(prev => { const next = new Map(prev); next.set(dir, name); return next; });
}

function SessionPickerModal() {
  // Fetch sessions whenever the modal is open — either because the user opened
  // it manually OR because there is no active session yet (forced choice).
  const isOpen = () => sessionPickerOpen() || needsSessionChoice();

  const [sessions] = createResource<SessionItem[], boolean>(
    isOpen,
    async (open: boolean) => {
      if (!open) return [];
      const res = await fetch("/sessions");
      if (!res.ok) return [];
      return res.json() as Promise<SessionItem[]>;
    },
  );

  const [searchQuery, setSearchQuery] = createSignal("");
  const [renamingDir, setRenamingDir] = createSignal<string | null>(null);
  const [renameValue, setRenameValue] = createSignal("");

  // The relative dir name of the current session (for marking "current")
  const currentDirName = () => {
    const d = state.sessionDir;
    return d ? (d.split("/").pop() ?? d) : "";
  };

  const filteredSessions = createMemo(() => {
    const all = sessions() ?? [];
    const deleted = deletedSessions();
    const renamed = renamedSessions();
    const patched = all
      .filter(s => !deleted.has(s.dir))
      .map(s => renamed.has(s.dir) ? { ...s, name: renamed.get(s.dir) } : s);
    const q = searchQuery().toLowerCase().trim();
    if (!q) return patched;
    return patched.filter(s =>
      (s.name ?? "").toLowerCase().includes(q) ||
      (s.description ?? "").toLowerCase().includes(q) ||
      s.dir.toLowerCase().includes(q)
    );
  });

  function resume(dir: string) {
    sendToServer({ type: "resume_session", sessionDir: dir });
    setSessionPickerOpen(false);
  }

  function deleteSession(dir: string, e: MouseEvent) {
    e.stopPropagation();
    sendToServer({ type: "delete_session", sessionDir: dir });
  }

  function startRename(dir: string, currentName: string | undefined, e: MouseEvent) {
    e.stopPropagation();
    setRenamingDir(dir);
    setRenameValue(currentName ?? "");
  }

  function commitRename(dir: string) {
    const name = renameValue().trim();
    if (name) {
      sendToServer({ type: "rename_session", sessionDir: dir, name });
    }
    setRenamingDir(null);
  }

  function cancelRename(e: MouseEvent) {
    e.stopPropagation();
    setRenamingDir(null);
  }

  function formatActivity(iso: string): string {
    try {
      const d = new Date(iso);
      if (isNaN(d.getTime())) return iso;
      return d.toLocaleString();
    } catch { return iso; }
  }

  return (
    <Show when={isOpen()}>
      {/* Backdrop never closes the modal — clicking outside is intentionally a no-op. */}
      <div class="modal-backdrop">
        <div class="modal session-picker-modal" data-testid="session-picker-modal">
          <div class="modal-header">
            <span class="modal-title">Sessions</span>
            <div class="modal-header-btns">
              <button
                class="session-picker-new"
                data-testid="session-picker-new"
                disabled={state.streaming || !state.connected}
                onClick={() => { newSession(); setSessionPickerOpen(false); }}
              >＋ New session</button>
              {/* Close button hidden when the user must make a choice (no session yet). */}
              <Show when={!needsSessionChoice()}>
                <button class="modal-close" onClick={() => { setRenamingDir(null); setSessionPickerOpen(false); }}>✕ close</button>
              </Show>
            </div>
          </div>

          <Show when={sessions.loading}>
              <div class="session-picker-loading">Loading sessions…</div>
            </Show>
            <Show when={!sessions.loading && (sessions() ?? []).length === 0}>
              <div class="session-picker-loading">No previous sessions found.</div>
            </Show>
            <Show when={!sessions.loading && (sessions() ?? []).length > 0}>
              <input
                class="session-picker-search"
                data-testid="session-picker-search"
                type="text"
                placeholder="Search sessions…"
                value={searchQuery()}
                onInput={(e) => setSearchQuery(e.currentTarget.value)}
              />
              <div class="session-picker-list" data-testid="session-picker-list">
                <For each={filteredSessions()}>
                  {(s) => {
                    const isCurrent = () => s.dir === currentDirName();
                    const isRenaming = () => renamingDir() === s.dir;
                    return (
                      <div class="session-picker-item" data-testid="session-picker-item"
                           classList={{ "session-picker-item-current": isCurrent() }}>
                        <div class="session-picker-item-header">
                          {/* Name area: inline editor when renaming, display otherwise */}
                          <Show when={isRenaming()}
                            fallback={
                              <div class="session-picker-name">
                                {s.name ?? <span class="session-picker-unnamed">(unnamed)</span>}
                                <Show when={isCurrent()}>
                                  <span class="session-picker-current-badge">current</span>
                                </Show>
                              </div>
                            }
                          >
                            <input
                              class="session-picker-rename-input"
                              data-testid="session-picker-rename-input"
                              type="text"
                              value={renameValue()}
                              placeholder="Session name…"
                              onInput={(e) => setRenameValue(e.currentTarget.value)}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") { e.preventDefault(); commitRename(s.dir); }
                                if (e.key === "Escape") { e.preventDefault(); setRenamingDir(null); }
                              }}
                              onClick={(e) => e.stopPropagation()}
                              ref={(el) => setTimeout(() => el?.focus(), 0)}
                            />
                          </Show>

                          {/* Action buttons */}
                          <div class="session-picker-item-btns">
                            <Show when={isRenaming()}
                              fallback={
                                <>
                                  <Show when={!isCurrent()}>
                                    <button class="session-picker-resume" data-testid="session-picker-resume"
                                            disabled={state.streaming}
                                            onClick={(e) => { e.stopPropagation(); resume(s.dir); }}
                                            title="Resume this session">Resume</button>
                                  </Show>
                                  <button class="session-picker-rename" data-testid="session-picker-rename"
                                          onClick={(e) => startRename(s.dir, s.name, e)}
                                          title="Rename this session">Rename</button>
                                  <button class="session-picker-delete" data-testid="session-picker-delete"
                                          disabled={state.streaming && isCurrent()}
                                          onClick={(e) => deleteSession(s.dir, e)} title="Delete session">✕</button>
                                </>
                              }
                            >
                              <button class="session-picker-save" data-testid="session-picker-save"
                                      onClick={(e) => { e.stopPropagation(); commitRename(s.dir); }}>Save</button>
                              <button class="session-picker-cancel-rename" data-testid="session-picker-cancel-rename"
                                      onClick={cancelRename}>Cancel</button>
                            </Show>
                          </div>
                        </div>

                        <div class="session-picker-meta">
                          <span class="session-picker-dir">{s.dir}</span>
                          <span class="session-picker-date">{formatActivity(s.lastActivity)}</span>
                        </div>
                        <Show when={s.description}>
                          <div class="session-picker-desc">{s.description}</div>
                        </Show>
                        <Show when={s.resumedFrom}>
                          <div class="session-picker-cont">↩ resumed from {s.resumedFrom}</div>
                        </Show>
                      </div>
                    );
                  }}
                </For>
              </div>
            </Show>
        </div>
      </div>
    </Show>
  );
}

function BottomPanel() {
  const hasMetrics = () => state.liveTurn !== null || state.lastTurnEnd !== null;

  return (
    <Show when={panelOpen()}>
      <div class="bottom-panel">
        <Show when={state.cwd}>
          <div class="bottom-panel-session" data-testid="session-panel">
            <span class="bp-label">cwd</span>
            <span class="bp-dir" data-testid="cwd-dir">{state.cwd}</span>
          </div>
        </Show>
        <Show when={hasMetrics()}>
          <MetricsTable />
        </Show>
      </div>
    </Show>
  );
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Reconnection banner
// ---------------------------------------------------------------------------

function ReconnectBanner() {
  return (
    <Show when={!state.connected && state.retryCount >= 2}>
      <div class="reconnect-banner" data-testid="reconnect-banner">
        ⚠ Cannot reach server — retrying… (attempt {state.retryCount})
        <br />
        Run <code>just server</code> in a terminal, then this page will reconnect automatically.
      </div>
    </Show>
  );
}

// ---------------------------------------------------------------------------
// ModelSelect — custom dropdown matching EffortSelect's look and behaviour
// ---------------------------------------------------------------------------

function ModelSelect() {
  const [open, setOpen] = createSignal(false);
  let ref!: HTMLDivElement;

  createEffect(() => {
    if (!open()) return;
    const handler = (e: MouseEvent) => {
      if (!ref.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    onCleanup(() => document.removeEventListener("mousedown", handler));
  });

  const options: Array<{ value: OmegaModel; label: string }> = [
    { value: "claude-sonnet-4-6", label: "Sonnet" },
    { value: "claude-opus-4-6",   label: "Opus"   },
  ];

  const currentLabel = () =>
    options.find(o => o.value === activeModel())?.label ?? activeModel();

  return (
    <div class="effort-select" ref={ref}>
      <button
        class="input-btn effort-trigger"
        disabled={state.streaming}
        onClick={() => setOpen(o => !o)}
      >
        {currentLabel()}
      </button>
      <Show when={open()}>
        <div class="effort-dropdown">
          <For each={options}>
            {(opt) => (
              <div
                class={"effort-option" + (opt.value === activeModel() ? " effort-option-selected" : "")}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => { handleModelChange(opt.value); setOpen(false); }}
              >
                {opt.label}
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------------------
// EffortSelect — custom dropdown that shows annotations only when open
// ---------------------------------------------------------------------------

function EffortSelect() {
  const [open, setOpen] = createSignal(false);
  let ref!: HTMLDivElement;

  // Close on outside click
  createEffect(() => {
    if (!open()) return;
    const handler = (e: MouseEvent) => {
      if (!ref.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    onCleanup(() => document.removeEventListener("mousedown", handler));
  });

  interface EffortOption { value: string; short: string; long: string; }

  const options = createMemo((): EffortOption[] => {
    const isOpus = activeModel() === "claude-opus-4-6";
    return [
      { value: "low",    short: "Low",    long: "Low" },
      { value: "medium", short: "Medium", long: isOpus ? "Medium" : "Medium (recommended)" },
      { value: "high",   short: "High",   long: "High (API default)" },
      ...(isOpus ? [{ value: "max", short: "Max", long: "Max" }] : []),
    ];
  });

  const current = () => options().find(o => o.value === activeEffort());

  return (
    <div class="effort-select" ref={ref}>
      <button
        class="input-btn effort-trigger"
        disabled={state.streaming}
        onClick={() => setOpen(o => !o)}
      >
        {current()?.short ?? activeEffort()}
      </button>
      <Show when={open()}>
        <div class="effort-dropdown">
          <For each={options()}>
            {(opt) => (
              <div
                class={"effort-option" + (opt.value === activeEffort() ? " effort-option-selected" : "")}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  sendToServer({ type: "set_effort", effort: opt.value as OmegaEffort });
                  setOpen(false);
                }}
              >
                {opt.long}
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------------------
// InputRow — Sessions · model · effort · textarea · Usage · status · Send
// ---------------------------------------------------------------------------

/**
 * If the text immediately before `cursor` contains an unbroken `@`-path token
 * (no whitespace between `@` and the cursor), return its start index and the
 * path-prefix string that follows the `@`. Returns null otherwise.
 */
function getAtToken(text: string, cursor: number): { start: number; prefix: string } | null {
  const before = text.slice(0, cursor);
  const match = before.match(/@(\S*)$/);
  if (!match) return null;
  return { start: before.length - match[0].length, prefix: match[1]! };
}

function InputRow() {
  let textareaRef!: HTMLTextAreaElement;
  let dropdownRef: HTMLDivElement | undefined;

  const [inputValue, setInputValue] = createSignal("");

  // Intercept Tab at the capture phase so the browser never shifts focus while
  // the completion dropdown is open.
  onMount(() => {
    function captureNavKeys(e: KeyboardEvent) {
      if (document.activeElement !== textareaRef) return;
      if (e.key === "Tab" && completionOpen()) { e.preventDefault(); }
    }
    window.addEventListener("keydown", captureNavKeys, { capture: true });
    onCleanup(() => window.removeEventListener("keydown", captureNavKeys, { capture: true }));
  });

  // ── @-path completion state ──
  const [completionItems, setCompletionItems] = createSignal<string[]>([]);
  const [completionHighlight, setCompletionHighlight] = createSignal(-1); // -1 = none
  const [completionOpen, setCompletionOpen] = createSignal(false);
  let fetchSeq = 0;

  function closeCompletion() {
    setCompletionOpen(false);
    setCompletionItems([]);
    setCompletionHighlight(-1);
  }

  async function queryCompletion(prefix: string) {
    const seq = ++fetchSeq;
    try {
      const res = await fetch(`/files?prefix=${encodeURIComponent(prefix)}`);
      if (seq !== fetchSeq) return;
      if (!res.ok) { closeCompletion(); return; }
      const items: string[] = await res.json() as string[];
      if (seq !== fetchSeq) return;
      setCompletionItems(items);
      setCompletionHighlight(-1);
      setCompletionOpen(items.length > 0);
    } catch {
      if (seq !== fetchSeq) return;
      closeCompletion();
    }
  }

  function moveHighlight(delta: number) {
    const items = completionItems();
    if (items.length === 0) return;
    const h = completionHighlight();
    // Wrap around: past last goes to first; at first goes to last.
    const next = delta > 0
      ? (h >= items.length - 1 ? 0 : h + 1)
      : (h <= 0 ? items.length - 1 : h - 1);
    setCompletionHighlight(next);
  }

  // Scroll the highlighted item into view whenever the highlight changes.
  createEffect(() => {
    const h = completionHighlight();
    if (h < 0 || !dropdownRef) return;
    dropdownRef.querySelectorAll<HTMLElement>(".fc-item")[h]?.scrollIntoView({ block: "nearest" });
  });

  function acceptCompletion(item: string) {
    const text = inputValue();
    const cursor = textareaRef.selectionStart ?? text.length;
    const token = getAtToken(text, cursor);
    if (!token) { closeCompletion(); return; }

    // Replace the @-token (from "@" through cursor) with "@" + selected item.
    const newText = text.slice(0, token.start) + "@" + item + text.slice(cursor);
    const newCursor = token.start + 1 + item.length;
    setInputValue(newText);

    // Restore cursor after SolidJS flushes the DOM update.
    setTimeout(() => {
      textareaRef.selectionStart = newCursor;
      textareaRef.selectionEnd   = newCursor;
      autoResize();
    }, 0);

    if (item.endsWith("/")) {
      void queryCompletion(item); // directory → drill deeper, keep dropdown open
    } else {
      closeCompletion();          // file → done
    }
  }

  function autoResize() {
    textareaRef.style.height = "auto";
    textareaRef.style.height = Math.min(textareaRef.scrollHeight, 240) + "px";
  }

  function send() {
    if (completionOpen()) return; // never send while the dropdown is visible
    const content = inputValue().trim();
    if (!content || state.streaming || !state.connected) return;
    sendToServer({ type: "message", content });
    setInputValue("");
    setTimeout(autoResize, 0);
  }

  function abort() {
    sendToServer({ type: "abort" });
  }

  function onKeyDown(e: KeyboardEvent) {
    if (completionOpen()) {
      if (e.key === "Escape") {
        e.preventDefault();
        closeCompletion();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const h = completionHighlight();
        if (h >= 0) acceptCompletion(completionItems()[h]!);
        else        closeCompletion();
        return; // never send while dropdown is open
      }
      if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
        e.preventDefault();
        moveHighlight(1);
        return;
      }
      if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
        e.preventDefault();
        moveHighlight(-1);
        return;
      }
      // "/" with a highlighted directory: accept it and drill in.
      if (e.key === "/" && completionHighlight() >= 0) {
        const item = completionItems()[completionHighlight()];
        if (item?.endsWith("/")) {
          e.preventDefault();
          acceptCompletion(item);
          return;
        }
      }
      // All other keys fall through to normal textarea handling
      // (typing narrows the filter via the onInput handler below).
    }

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
    setTimeout(autoResize, 0);
  }

  const statusDisplayClass = () =>
    "status-display " + (
      state.connecting  ? "status-connecting"
    : !state.connected ? "status-error"
    : state.retrying   ? "status-retrying"
    : state.streaming  ? "status-streaming"
    :                    "status-ready"
    );

  const omegaStatus = () =>
    state.connecting  ? "connecting"
    : !state.connected ? "disconnected"
    : state.retrying   ? "retrying"
    : state.streaming  ? "streaming"
    : "connected";

  const statusLabel = () =>
    state.connecting  ? (state.retryCount > 0 ? "Reconnecting…" : "Connecting…")
    : !state.connected ? "Disconnected"
    : state.retrying   ? "Retrying…"
    : state.streaming  ? "Streaming…"
    : "Ready";

  return (
    <div class="input-row">
      <button
        class="input-btn sessions-btn"
        data-testid="sessions-btn"
        data-session-dir={state.sessionDir ?? ""}
        onClick={() => setSessionPickerOpen(true)}
        title="Manage sessions"
      >Sessions</button>
      <Show when={activeModel()}>
        <ModelSelect />
        <EffortSelect />
      </Show>
      <div class="textarea-wrap">
        <Show when={completionOpen()}>
          <div class="fc-dropdown" ref={el => { dropdownRef = el; }}>
            <For each={completionItems()}>{(item, i) =>
              <div
                class={`fc-item${i() === completionHighlight() ? " fc-hl" : ""}${item.endsWith("/") ? " fc-dir" : ""}`}
                onMouseDown={e => { e.preventDefault(); acceptCompletion(item); }}
              >{item}</div>
            }</For>
          </div>
        </Show>
        <textarea
          ref={textareaRef}
          value={inputValue()}
          onInput={e => {
            setInputValue(e.currentTarget.value);
            autoResize();
            const cursor = e.currentTarget.selectionStart ?? e.currentTarget.value.length;
            const token = getAtToken(e.currentTarget.value, cursor);
            if (token !== null) void queryCompletion(token.prefix);
            else closeCompletion();
          }}
          onKeyDown={onKeyDown}
          onBlur={() => setTimeout(closeCompletion, 150)}
          placeholder="Message Omega… (@ for file, Enter to send, Shift+Enter for newline, ↑↓/Tab to navigate)"
          rows={1}
          disabled={!state.connected}
        />
      </div>
      <button
        class="input-btn panel-toggle-btn"
        data-testid="panel-toggle-btn"
        onClick={() => setPanelOpen(o => !o)}
        title={panelOpen() ? "Hide usage" : "Show usage"}
      >{panelOpen() ? "Hide usage" : "Show usage"}</button>
      <div class="status-row" data-testid="status-row">
        <span
          class={statusDisplayClass()}
          data-testid="omega-btn"
          data-status={omegaStatus()}
        >{statusLabel()}</span>
        <span class="status-label" data-testid="status-label">{statusLabel()}</span>
      </div>
      <Show when={state.streaming}
        fallback={
          <button class="input-btn send-btn" onClick={send} disabled={!state.connected}>Send</button>
        }
      >
        <button class="input-btn abort-btn" onClick={abort}>Abort</button>
      </Show>
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

  // Derive render groups from the flat event list.
  // These memos are created once at component-init time (correct SolidJS pattern).
  const renderGroups = createMemo(() => computeRenderGroups(state.events));
  const allLlmCalls = createMemo(() =>
    state.events.filter((ev): ev is ServerMessage & { type: "llm_call" } => ev.type === "llm_call")
  );

  // Start WebSocket on mount, clean up on unmount
  onMount(() => {
    // Expose dispatch + handleDisconnect for e2e tests (harmless in production)
    (window as any).__omegaDispatch = dispatch;
    (window as any).__omegaHandleDisconnect = handleDisconnect;
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
    // Track the flat event list length and ephemeral streaming content.
    // Any change to these signals new content in the feed.
    const _ = state.events.length;
    const __ = state.streamingText;
    const ___ = state.streamingThinking;
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
      <SessionPickerModal />
      <div class="feed-wrapper">
        <div class="feed" data-testid="feed" ref={feedRef} onScroll={onFeedScroll}>
          <ErrorBoundary fallback={(err) => (
            <div class="render-error" data-testid="render-error">
              <strong>Render error</strong>
              <pre>{err?.message ?? String(err)}</pre>
            </div>
          )}>
            <For each={renderGroups()}>{(group, groupIdx) => {
              const isLast = () => groupIdx() === renderGroups().length - 1;
              return <GroupView group={group} isLast={isLast()} allLlmCalls={allLlmCalls()} />;
            }}</For>
          </ErrorBoundary>
        </div>
        <Show when={!tailing()}>
          <button class="scroll-to-bottom" onClick={scrollToBottom} title="Scroll to latest">
            ↓
          </button>
        </Show>
      </div>
      <ReconnectBanner />
      <BottomPanel />
      <InputRow />
    </div>
  );
}
