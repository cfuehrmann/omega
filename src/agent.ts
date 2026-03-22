import Anthropic from "@anthropic-ai/sdk";
import { config } from "./config.js";
import { toolDefinitions, executeTool, type ToolResult } from "./tools.js";
import { getAuthToken, forceRefreshToken } from "./auth.js";

import {
  readSystemPromptAppend,
  systemPromptAppendPath,
} from "./system-prompt/append.js";
import { buildSystemPrompt as assembleSystemPrompt } from "./system-prompt/index.js";
import { appendContextMessage, buildContextRecord } from "./context-store.js";
import { appendEvent, DEFAULT_EVENTS_FILE } from "./event-store.js";
import type { OmegaEvent, StreamSignal } from "./events.js";

// --- Types ---

export interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface ModelResponse {
  id?: string;
  model?: string;
  type?: string;
  role?: string;
  content: Anthropic.ContentBlock[];
  stop_reason?: string | null;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens?: number | null;
    cache_read_input_tokens?: number | null;
    service_tier?: string | null;
    /** Present when server-side compaction fires — breakdown per iteration. */
    iterations?: unknown[] | null;
  };
}

export type { OmegaEvent, StreamSignal } from "./events.js";

// --- Request / response elision helpers ---

/** Count total characters across all text blocks in a system array or string. */
function charCount(value: unknown): number {
  if (typeof value === "string") return value.length;
  if (!Array.isArray(value)) return 0;
  return value.reduce(
    (n: number, b: any) =>
      n + (typeof b?.text === "string" ? b.text.length : 0),
    0,
  );
}

/**
 * Build a persisted elided summary of an Anthropic request.
 * Keeps all scalar fields verbatim; replaces system, tools, and messages
 * with compact descriptors so the shape is clear without the walls of text.
 */
function elideAnthropicRequest(req: {
  system: unknown;
  tools: unknown[];
  messages: unknown[];
  [key: string]: unknown;
}): Record<string, unknown> {
  const systemChars = charCount(req.system);
  const systemBlocks = Array.isArray(req.system) ? req.system.length : 1;
  const msgChars = JSON.stringify(req.messages).length;
  return {
    ...req,
    system: `[${systemBlocks} block${systemBlocks !== 1 ? "s" : ""}, ${systemChars} chars]`,
    tools: (req.tools as any[]).map((t: any) => ({
      name: t.name,
      description: `[${typeof t.description === "string" ? t.description.length : 0} chars]`,
      input_schema: `[elided]`,
      ...(t.cache_control ? { cache_control: t.cache_control } : {}),
    })),
    messages: `[${req.messages.length} message${req.messages.length !== 1 ? "s" : ""}, ${msgChars} chars]`,
  };
}

/**
 * Build a persisted elided summary of an Anthropic response.
 * Omits content (lives in context.jsonl); keeps all envelope fields verbatim.
 */
function elideAnthropicResponse(resp: ModelResponse): Record<string, unknown> {
  return {
    id: resp.id,
    type: resp.type,
    role: resp.role,
    model: resp.model,
    stop_reason: resp.stop_reason,
    usage: resp.usage,
    content: `[elided — use context hash]`,
  };
}

// --- Auto-approve logic ---

/** Always returns true — everything is auto-approved. No allowlist. */
export function isAutoApproved(_toolName: string, _toolInput: any): boolean {
  return true;
}

// --- Retry logic ---

/**
 * Return a shallow copy of the messages array where the last message has
 * cache_control: {type: "ephemeral"} on its last content block.
 * This creates a third cache breakpoint (after system and tools) so the
 * entire conversation prefix is cached.  Important for Opus which requires
 * ≥4096 prefix tokens before caching activates.
 */
function addCacheControlToLastMessage(
  messages: Anthropic.MessageParam[],
): Anthropic.MessageParam[] {
  if (messages.length === 0) return messages;
  const result = [...messages];
  const last = result[result.length - 1];

  // Normalise content to an array of blocks
  let blocks: Anthropic.ContentBlockParam[];
  if (typeof last.content === "string") {
    blocks = [{ type: "text" as const, text: last.content }];
  } else if (Array.isArray(last.content)) {
    blocks = [...(last.content as Anthropic.ContentBlockParam[])];
  } else {
    return result; // unexpected shape — leave untouched
  }

  if (blocks.length === 0) return result;

  // Add cache_control to the last block
  blocks[blocks.length - 1] = {
    ...blocks[blocks.length - 1],
    cache_control: { type: "ephemeral" },
  } as any;

  result[result.length - 1] = { ...last, content: blocks };
  return result;
}

async function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error("aborted"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(new Error("aborted"));
    };
    signal?.addEventListener("abort", onAbort);
  });
}

/**
 * Returns true if the error is a "context too long" refusal from Claude Max's
 * OAuth endpoint. Claude Max returns 429 (not 400) with the message
 * "Extra usage is required for long context requests." when the prompt exceeds
 * the context window allowed for the account tier. Retrying with the same
 * payload is futile — this must be treated as non-retryable.
 */
export function isContextTooLong(err: any): boolean {
  if (!err) return false;
  const status = err.status ?? err.statusCode;
  if (status !== 429) return false;
  if (typeof err.message !== "string") return false;
  return err.message.includes(
    "Extra usage is required for long context requests",
  );
}

export function isRetryable(err: any): boolean {
  if (!err) return false;
  // A 429 from Claude Max meaning "prompt too long for your tier" is NOT transient.
  // Exclude it before the blanket status check so we don't retry fruitlessly.
  if (isContextTooLong(err)) return false;
  const status = err.status ?? err.statusCode;
  if (status === 429 || status === 529 || status === 500 || status === 503)
    return true;
  if (typeof err.message === "string") {
    const msg: string = err.message;
    // The Anthropic SDK throws this when the server restarts a stream mid-flight
    // (a new message_start arrives before message_stop). No HTTP status code —
    // it's thrown internally by MessageStream. Treat as transient and retry.
    if (msg.includes("Unexpected event order")) return true;
    // Bun throws this when the TCP connection to the API server is closed
    // unexpectedly (idle connection recycled, server-side keepalive timeout,
    // network blip). No HTTP status code — it's a fetch-level error.
    // Seen in diagnosis/2026-02-23T18-36-29-982Z.json during world-state fold.
    if (msg.includes("socket connection was closed unexpectedly")) return true;
    // Node/Bun also surfaces TCP resets as ECONNRESET via fetch().
    if (msg.includes("ECONNRESET")) return true;
  }
  return false;
}

/**
 * Returns true if the error is an OAuth token auth failure that can be
 * recovered by refreshing the token. Covers two cases:
 *  - 401 authentication_error: token expired (normal session timeout)
 *  - 403 permission_error with "revoked": token was explicitly revoked
 *    (e.g. the user re-authenticated in another session)
 */
export function isAuthExpired(err: any): boolean {
  if (!err) return false;
  const status = err.status ?? err.statusCode;
  const msg: string =
    typeof err.message === "string" ? err.message : JSON.stringify(err);
  if (status === 401) {
    return (
      msg.includes("authentication_error") ||
      msg.includes("OAuth token has expired")
    );
  }
  if (status === 403) {
    // Anthropic 403 body: {"type":"error","error":{"type":"permission_error","message":"OAuth token has been revoked..."}}
    return (
      msg.includes("revoked") &&
      (msg.includes("permission_error") || msg.includes("OAuth token"))
    );
  }
  return false;
}

// --- Context window management ---
// Truncates conversation history to stay within the token budget.
// Always preserves the first user message (the original task) and the most
// recent N turns.

function getAnthropicRetryDelayMs(
  _err: any,
  attempt: number,
  baseMs: number,
  maxMs: number,
): number {
  const jitter = Math.random() * 0.2 + 0.9; // 0.9–1.1
  const delay = baseMs * Math.pow(2, attempt) * jitter;
  return Math.min(Math.round(delay), maxMs);
}

// --- Stream event processing (extracted for testability) ---

/** Process raw Anthropic stream events into AgentEvents.
 *  This is the inner loop of sendMessage, extracted so it can be tested
 *  without a real API connection. */
export function processStreamEvents(
  streamEvents: Iterable<any>,
): (OmegaEvent | StreamSignal)[] {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for (const event of streamEvents) {
    if (event.type === "content_block_delta") {
      if (event.delta.type === "text_delta") {
        events.push({ type: "text", text: event.delta.text });
      } else if (event.delta.type === "thinking_delta") {
        events.push({ type: "thinking", text: event.delta.thinking });
      }
    }
  }
  return events;
}

// --- Agent ---

/**
 * A StreamProvider is a function that calls the LLM provider API
 * (or a mock in tests) and returns an object with an async iterator of
 * raw stream events and a finalMessage() method.
 *
 * By accepting a StreamProvider in the constructor, the Agent can be
 * tested without hitting the real LLM provider API.
 *
 * NOTE: This type is referenced by name in .omega/system-prompt-append.md.
 * If you rename it, update that file too.
 */
export type StreamProvider = (params: {
  model: string;
  max_tokens: number;
  system: string | Anthropic.TextBlockParam[];
  tools: Anthropic.Tool[];
  messages: Anthropic.MessageParam[];
  /** Beta headers to include with this request (e.g. compact-2026-01-12). */
  betas?: string[];
  /** Server-side context management directives (e.g. compaction). */
  context_management?: Record<string, unknown>;
  /** Enable extended thinking. When set, the model reasons before responding. */
  thinking?: { type: "adaptive" };
}) => Promise<{
  [Symbol.asyncIterator](): AsyncIterator<any>;
  finalMessage(): Promise<
    Anthropic.Message | Anthropic.Beta.Messages.BetaMessage
  >;
}>;

export class Agent {
  private client: Anthropic;
  private compactedContextHistory: Anthropic.MessageParam[] = [];
  /** Parallel to compactedContextHistory — stores the 8-char content hash of each stored record. */
  private compactedContextHashes: string[] = [];
  public sessionInputTokens = 0;
  public sessionOutputTokens = 0;
  public sessionCacheCreationTokens = 0;
  public sessionCacheReadTokens = 0;

  private authMode: "api-key" | "oauth" = "api-key";
  private activeModel: string = config.model;
  /** True once session_start has been logged — prevents duplicate on reconnect. */
  private sessionStartLogged = false;

  public readonly sessionId: string;
  private readonly retryBaseMs = Number(
    process.env.OMEGA_RETRY_BASE_MS ?? 1000,
  );
  private readonly retryMaxMs = Number(process.env.OMEGA_RETRY_MAX_MS ?? 60000);
  private readonly retryMaxAttempts = Number(
    process.env.OMEGA_RETRY_ATTEMPTS ?? 5,
  );

  /** Context JSONL file path. null = disabled (tests). undefined = use production default. */
  private readonly contextFile: string | null | undefined;

  /** Events JSONL file path. null = disabled (tests). undefined = use production default. */
  private readonly eventsFile: string | null | undefined;

  /** Content of .omega/system-prompt-append.md, injected into system prompt at session start. */
  private systemPromptAppendContent: string | null = null;

  /** Optional injectable stream provider (used in tests). */
  private readonly streamProvider: StreamProvider | undefined;

  /**
   * Production: new Agent()
   *   — uses real Anthropic client, context appended to .omega/sessions/<ts>/context.jsonl
   * Test: new Agent(mockProvider, contextFile, eventsFile)
   *   — uses mock provider; context file and events file are disabled unless
   *     explicit paths are given.
   */
  constructor(
    streamProvider?: StreamProvider,
    contextFile?: string | null,
    eventsFile?: string | null,
  ) {
    // Will be initialized in init()
    this.client = new Anthropic();
    this.sessionId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    this.streamProvider = streamProvider;

    // Layer c: in test env, any unspecified file path defaults to null (disabled).
    const inTestEnv = process.env.OMEGA_TEST === "1";

    // Context file: disable in test env unless explicitly set; or if mock provider given.
    if (
      (inTestEnv || streamProvider !== undefined) &&
      contextFile === undefined
    ) {
      this.contextFile = null;
    } else {
      this.contextFile = contextFile;
    }
    // Events file: disable in test env unless explicitly set; or if mock provider given.
    if (
      (inTestEnv || streamProvider !== undefined) &&
      eventsFile === undefined
    ) {
      this.eventsFile = null;
    } else {
      this.eventsFile = eventsFile;
    }
  }

  /** Resolve the events file path (null = disabled). */
  private resolveEventsFile(): string | null {
    return this.eventsFile === undefined
      ? DEFAULT_EVENTS_FILE
      : this.eventsFile;
  }

  /** Serial write queue — ensures events are appended in the order logEvent() is called, regardless of whether the caller awaits. */
  private logQueue: Promise<void> = Promise.resolve();

  /** Append an OmegaEvent to the events file. Calls are serialised through logQueue, so ordering is guaranteed even for fire-and-forget callers. Errors silently dropped. */
  private logEvent(event: OmegaEvent): Promise<void> {
    const path = this.resolveEventsFile();
    if (path === null) return Promise.resolve();
    this.logQueue = this.logQueue.then(() =>
      appendEvent(event, path).catch(() => {}),
    );
    return this.logQueue;
  }

  /**
   * Write a session_end event and await the flush.
   * Call this at clean shutdown before process.exit().
   * A crash/SIGKILL will leave no session_end — that absence is the crash signal.
   */
  async emitSessionEnd(
    outcome: "clean" | "error",
    reason?: string,
  ): Promise<void> {
    await this.logEvent({
      type: "session_end",
      ts: new Date().toISOString(),
      outcome,
      ...(reason ? { reason } : {}),
    });
  }

  /**
   * Append a message to compactedContextHistory, compute and store its content hash,
   * and fire-and-forget the context file write. Returns the hash.
   */
  private async appendToHistory(msg: Anthropic.MessageParam): Promise<string> {
    this.compactedContextHistory.push(msg);
    // Compute hash (needed for contextHashes) — file write is fire-and-forget
    if (this.contextFile !== null) {
      const hash = await appendContextMessage(
        msg,
        this.contextFile ?? undefined,
      );
      this.compactedContextHashes.push(hash);
      return hash;
    } else {
      // No file write, but still need a hash for contextHashes cross-referencing
      const record = await buildContextRecord(msg);
      this.compactedContextHashes.push(record.hash);
      return record.hash;
    }
  }

  async init(): Promise<string> {
    // Auth flow (matching pi-ai's anthropic.js):
    // OAuth via claude.ai → access_token (sk-ant-oat-...)
    // Pass as authToken (Bearer auth) with Claude Code beta headers.
    // The API requires claude-code-20250219 + oauth-2025-04-20 betas.
    const accessToken = await getAuthToken();
    if (accessToken) {
      this.client = new Anthropic({
        apiKey: null as any,
        authToken: accessToken,
        defaultHeaders: {
          accept: "application/json",
          "anthropic-dangerous-direct-browser-access": "true",
          "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
          "user-agent": "claude-cli/2.1.2 (external, cli)",
          "x-app": "cli",
        },
      });
      this.authMode = "oauth";
      if (!this.sessionStartLogged) {
        this.sessionStartLogged = true;
        this.logEvent({
          type: "session_start",
          ts: new Date().toISOString(),
          sessionId: this.sessionId,
          model: this.activeModel,
          provider: "anthropic",
          authMode: "claude-max",
          systemPrompt: this.buildSystemPrompt(),
        });
      }
      return "Claude Max";
    } else if (process.env.ANTHROPIC_API_KEY) {
      this.client = new Anthropic();
      this.authMode = "api-key";
      if (!this.sessionStartLogged) {
        this.sessionStartLogged = true;
        this.logEvent({
          type: "session_start",
          ts: new Date().toISOString(),
          sessionId: this.sessionId,
          model: this.activeModel,
          provider: "anthropic",
          authMode: "api-key",
          systemPrompt: this.buildSystemPrompt(),
        });
      }
      return "api-key (pay-per-token ⚠)";
    } else {
      throw new Error(
        "No authentication found. Run `bun run src/login.ts` to authenticate with Claude Max, or set ANTHROPIC_API_KEY.",
      );
    }
  }

  /** Build the system prompt from all parts. */
  buildSystemPrompt(): string {
    return assembleSystemPrompt({
      cwd: process.cwd(),
      maxOutputTokens: config.maxOutputTokens,
      appendContent: this.systemPromptAppendContent,
    });
  }

  getAuthMode(): string {
    return this.authMode;
  }

  getActiveModel(): string {
    return this.activeModel;
  }

  /**
   * Force-refresh the OAuth token and reinitialize the Anthropic client.
   * Call this after receiving a 401 authentication_error mid-session.
   * Returns true if reinit succeeded, false if refresh failed.
   */
  async reinitAuth(): Promise<boolean> {
    if (this.authMode !== "oauth") return false; // nothing to refresh for API keys
    const newToken = await forceRefreshToken();
    if (!newToken) return false;
    this.client = new Anthropic({
      apiKey: null as any,
      authToken: newToken,
      defaultHeaders: {
        accept: "application/json",
        "anthropic-dangerous-direct-browser-access": "true",
        "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
        "user-agent": "claude-cli/2.1.2 (external, cli)",
        "x-app": "cli",
      },
    });
    this.logEvent({ type: "oauth_refreshed", ts: new Date().toISOString() });
    return true;
  }

  getCompactedContextHistory(): readonly Anthropic.MessageParam[] {
    return this.compactedContextHistory;
  }

  /** Exposed for testing only — allows verification that the hashes array stays in sync. */
  getCompactedContextHashes(): readonly string[] {
    return this.compactedContextHashes;
  }

  /**
   * Get a StreamProvider wrapping the real Anthropic client (or the injected
   * mock, in tests).
   */
  private getStreamProvider(): StreamProvider {
    if (this.streamProvider) return this.streamProvider;
    const client = this.client;
    return async (params) => client.beta.messages.stream(params as any);
  }

  /**
   * Load .omega/system-prompt-append.md from disk into memory so it can be
   * injected into the system prompt. Call once at session start, after init().
   *
   * @param path  Optional override for the file path. Defaults to
   *              `.omega/system-prompt-append.md` in the current working
   *              directory. Pass an explicit path in tests to avoid touching
   *              the real project file.
   */
  async loadSystemPromptAppend(
    path: string = systemPromptAppendPath(),
  ): Promise<void> {
    try {
      this.systemPromptAppendContent = await readSystemPromptAppend(path);
    } catch {
      this.systemPromptAppendContent = null;
    }
  }

  async *sendMessage(
    userMessage: string,
    _confirmTool: (name: string, input: any) => Promise<boolean>,
    signal?: AbortSignal,
  ): AsyncGenerator<OmegaEvent | StreamSignal> {
    if (userMessage.startsWith("/")) {
      const cmd = userMessage.trim().toLowerCase();
      if (cmd === "/sonnet") {
        this.activeModel = "claude-sonnet-4-6";
        const ev: OmegaEvent = {
          type: "model_changed",
          ts: new Date().toISOString(),
          model: "claude-sonnet-4-6",
        };
        this.logEvent(ev);
        yield ev;
      } else if (cmd === "/opus") {
        this.activeModel = "claude-opus-4-6";
        const ev: OmegaEvent = {
          type: "model_changed",
          ts: new Date().toISOString(),
          model: "claude-opus-4-6",
        };
        this.logEvent(ev);
        yield ev;
      } else {
        yield {
          type: "agent_error",
          ts: new Date().toISOString(),
          error: `Unknown command: ${userMessage}`,
        };
      }
      return;
    }

    await this.appendToHistory({ role: "user", content: userMessage });
    const userMessageEvent: OmegaEvent = {
      type: "user_message",
      ts: new Date().toISOString(),
      content: userMessage,
    };
    this.logEvent(userMessageEvent);
    yield userMessageEvent;

    // Cumulative totals across all API calls in this user turn
    let totalInputTokens = 0;
    let totalOutputTokens = 0;
    let totalCacheCreationTokens = 0;
    let totalCacheReadTokens = 0;

    // Agentic loop: keep going while the model wants to use tools
    let continueLoop = true;
    let activeModel = this.activeModel;
    while (continueLoop) {
      continueLoop = false;

      let turnInputTokens = 0;
      let turnOutputTokens = 0;
      let assembledText = "";
      let assembledTextTs: string | null = null;
      let assembledThinking = "";
      /** True when we are inside a thinking block (between block_start and block_stop). */
      let inThinkingBlock = false;

      // Build system prompt (core instructions + system-prompt-append if loaded).
      const systemPrompt = this.buildSystemPrompt();

      activeModel = this.activeModel;

      // Build cached system blocks and cached tools for Anthropic prompt caching.
      // The system prompt is split into blocks with cache_control on the last block,
      // so the entire system prompt (including any appended content) is cached after the first call.
      // The last tool definition also gets cache_control to cache all tool definitions.
      //
      // The first block is a plain billing/attribution header (no cache_control) that
      // Anthropic's infrastructure uses for client identification — matching the pattern
      // used by Claude Code.  It must come before the cached prompt block.
      const billingHeaderText = `x-anthropic-billing-header: cc_version=1.0.0; cc_entrypoint=omega; cch=00000;`;
      const systemBlocks: Anthropic.TextBlockParam[] = [
        {
          type: "text",
          text: billingHeaderText,
          // No cache_control — this block is intentionally uncached.
        },
        {
          type: "text",
          text: systemPrompt,
          cache_control: { type: "ephemeral" },
        },
      ];
      const cachedTools: Anthropic.Tool[] =
        toolDefinitions.length > 0
          ? [
              ...toolDefinitions.slice(0, -1),
              {
                ...(toolDefinitions[toolDefinitions.length - 1] as any),
                cache_control: { type: "ephemeral" },
              },
            ]
          : toolDefinitions;

      // All messages are sent verbatim — no in-turn trimming.
      // addCacheControlToLastMessage adds cache_control to the last message for Anthropic caching.
      const sentContext = this.compactedContextHistory;
      const cachedMessages = addCacheControlToLastMessage(sentContext);

      // contextHashes: all hashes in order, one per message in compactedContextHistory.
      const contextHashes = [...this.compactedContextHashes];

      // Build the full request params once — used for both the audit event and
      // each retry attempt. Defined here so the llm_call summary reflects the
      // exact payload sent to the API (pass-through, not a whitelist).
      const streamParams = {
        model: activeModel,
        max_tokens: config.maxOutputTokens,
        system: systemBlocks,
        tools: cachedTools,
        messages: cachedMessages,
        betas: ["compact-2026-01-12"],
        context_management: {
          edits: [
            {
              type: "compact_20260112",
              trigger: { type: "input_tokens", value: 150_000 },
            },
          ],
        },
        thinking: { type: "adaptive" as const },
      };

      // Emit llm_call with a persisted elided request summary.
      {
        const llmCallEv: OmegaEvent = {
          type: "llm_call",
          ts: new Date().toISOString(),
          provider: "anthropic",
          url: "https://api.anthropic.com/v1/messages",
          model: activeModel,
          contextHashes,
          cacheBreakpointIndex:
            contextHashes.length > 0 ? contextHashes.length - 1 : null,
          requestBytes: JSON.stringify(streamParams).length,
          requestSummary: elideAnthropicRequest(streamParams),
        };
        this.logEvent(llmCallEv);
        yield llmCallEv;
      }

      // Call API with retry
      let response: ModelResponse | null = null;
      let lastError: any = null;

      for (let attempt = 0; attempt < this.retryMaxAttempts; attempt++) {
        try {
          assembledText = "";
          assembledThinking = "";
          inThinkingBlock = false;
          const stream = this.streamProvider
            ? await this.streamProvider(streamParams)
            : this.client.beta.messages.stream(streamParams as any);

          let aborted = false;
          for await (const event of stream) {
            if (signal?.aborted) {
              aborted = true;
              break;
            }
            if (event.type === "content_block_start") {
              if (event.content_block?.type === "thinking") {
                // If we already have thinking content, insert a divider between blocks.
                if (assembledThinking.length > 0) {
                  assembledThinking += "\n\n---\n\n";
                }
                inThinkingBlock = true;
              }
            } else if (event.type === "content_block_stop") {
              if (inThinkingBlock) {
                inThinkingBlock = false;
              }
            } else if (event.type === "content_block_delta") {
              if (event.delta.type === "text_delta") {
                if (assembledTextTs === null)
                  assembledTextTs = new Date().toISOString();
                assembledText += event.delta.text;
                yield { type: "text", text: event.delta.text };
              } else if (event.delta.type === "thinking_delta") {
                assembledThinking += event.delta.thinking;
                yield { type: "thinking", text: event.delta.thinking };
              }
            }
            // Compaction content blocks arrive as a single delta (no incremental
            // streaming). We don't yield them as text — they're structural.
          }

          if (aborted) {
            // Don't add the partial assistant turn to history.
            // The user message stays — it was real input.
            const interruptEv: OmegaEvent = {
              type: "turn_interrupted",
              ts: new Date().toISOString(),
            };
            this.logEvent(interruptEv);
            yield interruptEv;
            return;
          }

          response = (await stream.finalMessage()) as unknown as ModelResponse;
          lastError = null;
          break;
        } catch (err: any) {
          lastError = err;

          if (isAuthExpired(err) && attempt === 0) {
            // OAuth token expired or revoked mid-session — try to refresh and retry once
            const expiredEv: OmegaEvent = {
              type: "oauth_token_expired",
              ts: new Date().toISOString(),
              attempt: attempt + 1,
              httpStatus: err.status ?? err.statusCode,
            };
            this.logEvent(expiredEv);
            yield expiredEv;
            const reauthed = await this.reinitAuth();
            if (reauthed) {
              // reinitAuth already logs oauth_refreshed; yield it for the UI too
              yield { type: "oauth_refreshed", ts: new Date().toISOString() };
              // Loop continues — the next iteration will use the fresh client
            } else {
              const llmErrEv: OmegaEvent = {
                type: "llm_error",
                ts: new Date().toISOString(),
                provider: "anthropic",
                url: "https://api.anthropic.com/v1/messages",
                error: err.message ?? String(err),
                httpStatus: err.status ?? err.statusCode,
              };
              this.logEvent(llmErrEv);
              yield llmErrEv;
              const authFailEv: OmegaEvent = {
                type: "agent_error",
                ts: new Date().toISOString(),
                error:
                  "OAuth token expired and refresh failed. Run `bun run src/login.ts` to re-authenticate.",
              };
              this.logEvent(authFailEv);
              yield authFailEv;
              return;
            }
          } else if (isRetryable(err) && attempt < this.retryMaxAttempts - 1) {
            const waitMs = getAnthropicRetryDelayMs(
              err,
              attempt,
              this.retryBaseMs,
              this.retryMaxMs,
            );
            const retryEv: OmegaEvent = {
              type: "llm_retry",
              ts: new Date().toISOString(),
              attempt: attempt + 1,
              provider: "anthropic",
              httpStatus: err.status ?? err.statusCode,
              waitMs,
              error: err.message,
            };
            this.logEvent(retryEv);
            yield retryEv;
            yield {
              type: "agent_error",
              ts: new Date().toISOString(),
              error: `${err.message ?? err}. Retrying in ${Math.round(waitMs / 1000)}s... (${attempt + 1}/${this.retryMaxAttempts})`,
            };
            await sleep(waitMs, signal);
          } else {
            // Non-retryable error (includes prompt-too-long — no retry, no trimming).
            // Write a diagnostic snapshot so the next session has hard data.
            const isContextOverflow =
              (err.status === 400 &&
                typeof err.message === "string" &&
                err.message.includes("prompt is too long")) ||
              isContextTooLong(err);
            const llmErrEv: OmegaEvent = {
              type: "llm_error",
              ts: new Date().toISOString(),
              provider: "anthropic",
              url: "https://api.anthropic.com/v1/messages",
              error: err.message ?? String(err),
              httpStatus: err.status ?? err.statusCode,
            };
            this.logEvent(llmErrEv);
            yield llmErrEv;
            if (isContextOverflow) {
              const overflowEv: OmegaEvent = {
                type: "agent_error",
                ts: new Date().toISOString(),
                error: "Context too large to send. Start a fresh focused turn.",
              };
              this.logEvent(overflowEv);
              yield overflowEv;
            } else if (isRetryable(err)) {
              const rateLimitEv: OmegaEvent = {
                type: "agent_error",
                ts: new Date().toISOString(),
                error: "Anthropic rate limit. Try again shortly.",
              };
              this.logEvent(rateLimitEv);
              yield rateLimitEv;
            } else {
              const apiErrEv: OmegaEvent = {
                type: "agent_error",
                ts: new Date().toISOString(),
                error: `API error: ${err.message ?? err}`,
              };
              this.logEvent(apiErrEv);
              yield apiErrEv;
            }
            return;
          }
        }
      }

      if (!response) {
        yield {
          type: "agent_error",
          ts: new Date().toISOString(),
          error: `API error after 5 retries: ${lastError?.message ?? lastError}`,
        };
        return;
      }

      // Track tokens
      turnInputTokens = response.usage.input_tokens;
      turnOutputTokens = response.usage.output_tokens;
      const turnCacheCreation = response.usage.cache_creation_input_tokens ?? 0;
      const turnCacheRead = response.usage.cache_read_input_tokens ?? 0;
      this.sessionInputTokens += turnInputTokens;
      this.sessionOutputTokens += turnOutputTokens;
      this.sessionCacheCreationTokens += turnCacheCreation;
      this.sessionCacheReadTokens += turnCacheRead;

      // Accumulate turn-level totals
      totalInputTokens += turnInputTokens;
      totalOutputTokens += turnOutputTokens;
      totalCacheCreationTokens += turnCacheCreation;
      totalCacheReadTokens += turnCacheRead;

      // Detect server-side compaction: a compaction block in the response means
      // the server summarised the history. Prune compactedContextHistory and
      // compactedContextHashes so the next call only sends from the compaction
      // block onward. The compaction block sits at index 0 of the content array.
      const compactionBlockIndex = response.content.findIndex(
        (b: any) => b.type === "compaction",
      );
      if (compactionBlockIndex !== -1) {
        // The API drops everything prior to the compaction block server-side,
        // but we also prune locally so our local array stays in sync.
        // The compaction block is part of this assistant message, which we are
        // about to append. After appending, the local array should start from
        // this message — so clear all prior history.
        this.compactedContextHistory = [];
        this.compactedContextHashes = [];

        // Emit a compacted event — full usage object preserved verbatim.
        const compactedEv: OmegaEvent = {
          type: "compacted",
          ts: new Date().toISOString(),
          usage: response.usage,
        };
        await this.logEvent(compactedEv);
        yield compactedEv;
      }

      // Add assistant response to history; capture hash for llm_response + tool_call events.
      // appendToHistory is awaited so the context.jsonl record is on disk before
      // logEvent(llm_response) fires (which carries contextHash as a FK).
      const assistantHash = await this.appendToHistory({
        role: "assistant",
        content: response.content,
      });
      const llmResponseEvent: OmegaEvent = {
        type: "llm_response",
        ts: new Date().toISOString(),
        stopReason: response.stop_reason ?? "unknown",
        usage: {
          input_tokens: response.usage.input_tokens ?? 0,
          output_tokens: response.usage.output_tokens,
          cache_creation_input_tokens:
            response.usage.cache_creation_input_tokens ?? undefined,
          cache_read_input_tokens:
            response.usage.cache_read_input_tokens ?? undefined,
          service_tier: response.usage.service_tier ?? undefined,
        },
        contextHash: assistantHash,
        ...(assembledText
          ? {
              text: assembledText,
              streamingStart: assembledTextTs ?? undefined,
            }
          : {}),
        ...(assembledThinking ? { thinking: assembledThinking } : {}),
        responseSummary: elideAnthropicResponse(response),
      };
      // Await so llm_response is flushed before any tool_call events fire.
      // tool_call is causally downstream of llm_response; without await the
      // two fire-and-forget writes race and tool_call can land first in events.jsonl.
      await this.logEvent(llmResponseEvent);
      yield llmResponseEvent;

      // Process tool calls if any
      const toolUseBlocks = response.content.filter(
        (b): b is Anthropic.ToolUseBlock => b.type === "tool_use",
      );

      // --- BUG-1 guard: max_tokens mid-tool-call ---
      // If the LLM was cut off by max_tokens while emitting tool_use blocks, the
      // assistant message (already appended to compactedContextHistory above) contains
      // dangling tool_use blocks with no matching tool_result. Anthropic rejects
      // this with a 400 on the very next API call, permanently bricking the session.
      //
      // Fix: synthesise error tool_result entries for every dangling tool_use,
      // append them to history immediately, emit tool_result events, and then
      // emit an agent_error explaining what happened. The turn ends here (no
      // continueLoop = true), but the context is well-formed and the next user
      // message will succeed.
      if (toolUseBlocks.length > 0 && response.stop_reason === "max_tokens") {
        const syntheticResults: Anthropic.ToolResultBlockParam[] =
          toolUseBlocks.map((b) => ({
            type: "tool_result" as const,
            tool_use_id: b.id,
            content: `[not executed: max_tokens stop — output budget (${config.maxOutputTokens} tokens) was exhausted while generating this tool call's arguments — retry with a smaller write_file or use edit_file instead]`,
            is_error: true,
          }));
        const syntheticResultHash = await this.appendToHistory({
          role: "user",
          content: syntheticResults,
        });
        for (const toolUse of toolUseBlocks) {
          const syntheticResultEvent: OmegaEvent = {
            type: "tool_result",
            ts: new Date().toISOString(),
            id: toolUse.id,
            name: toolUse.name,
            isError: true,
            durationMs: 0,
            output:
              "[not executed: max_tokens stop — output budget exhausted while generating tool call arguments]",
            contextHash: syntheticResultHash,
          };
          this.logEvent(syntheticResultEvent);
          yield syntheticResultEvent;
        }
        const toolNames = toolUseBlocks.map((b) => b.name).join(", ");
        const truncErr =
          `Output budget exhausted (max_tokens) while generating tool call input for [${toolNames}] — the tool was not executed. ` +
          `This means the tool call arguments alone exceeded the ${config.maxOutputTokens}-token output budget. ` +
          `To avoid this: break large write_file calls into a skeleton + edit_file extensions; ` +
          `never attempt to write a file longer than ~500 lines in a single write_file call. ` +
          `The session context is intact — retry with a smaller approach.`;
        const truncErrEvent: OmegaEvent = {
          type: "agent_error",
          ts: new Date().toISOString(),
          error: truncErr,
        };
        await this.logEvent(truncErrEvent);
        yield truncErrEvent;
        // Do NOT set continueLoop = true — turn ends here.
      }

      if (toolUseBlocks.length > 0 && response.stop_reason === "tool_use") {
        const toolResults: Anthropic.ToolResultBlockParam[] = [];

        // Emit all tool_call events first, then execute all tools in parallel,
        // then emit all tool_result events. This reduces wall-clock latency when
        // the model returns multiple tool_use blocks in one response.
        for (const toolUse of toolUseBlocks) {
          const toolCallEvent: OmegaEvent = {
            type: "tool_call",
            ts: new Date().toISOString(),
            id: toolUse.id,
            name: toolUse.name,
            input: toolUse.input,
            contextHash: assistantHash,
          };
          this.logEvent(toolCallEvent);
          yield toolCallEvent;
        }

        // Execute all tools concurrently
        const results = await Promise.all(
          toolUseBlocks.map((toolUse) =>
            executeTool(toolUse.name, toolUse.input),
          ),
        );

        for (let i = 0; i < toolUseBlocks.length; i++) {
          const toolUse = toolUseBlocks[i];
          const result = results[i];

          toolResults.push({
            type: "tool_result",
            tool_use_id: toolUse.id,
            content: result.output,
            is_error: result.isError,
          });
        }

        // Add tool results to history; capture hash for tool_result events
        const toolResultHash = await this.appendToHistory({
          role: "user",
          content: toolResults,
        });
        for (let i = 0; i < toolUseBlocks.length; i++) {
          const toolUse = toolUseBlocks[i];
          const result = results[i];
          const toolResultEvent: OmegaEvent = {
            type: "tool_result",
            ts: new Date().toISOString(),
            id: toolUse.id,
            name: toolUse.name,
            isError: result.isError,
            durationMs: result.durationMs,
            output: result.output,
            contextHash: toolResultHash,
          };
          this.logEvent(toolResultEvent);
          yield toolResultEvent;
        }
        continueLoop = true;
      }
    }

    // Emit one turn_end after all API calls complete
    const turnEndMetrics: TurnMetrics = {
      inputTokens: totalInputTokens,
      outputTokens: totalOutputTokens,
      cacheCreationTokens: totalCacheCreationTokens,
      cacheReadTokens: totalCacheReadTokens,
    };
    const turnEndEvent: OmegaEvent = {
      type: "turn_end",
      ts: new Date().toISOString(),
      metrics: turnEndMetrics,
    };
    this.logEvent(turnEndEvent);
    yield turnEndEvent;
  }
}
