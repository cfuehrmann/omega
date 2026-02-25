import Anthropic from "@anthropic-ai/sdk";
import { config } from "./config.js";
import { toolDefinitions, executeTool, formatToolCall, type ToolResult } from "./tools.js";
import { getAuthToken, forceRefreshToken } from "./auth.js";
import { logger, flushLog, startup, makeLogEntry } from "./logger.js";
import { writeDiagnostic } from "./diagnosis.js";
import { callOpenAi, buildOpenAiRequest, getOpenAiUrl } from "./openai.js";
import { compactWorldState, compactHistory } from "./compaction.js";
import { readWorldState, writeWorldState, projectWorldStatePath } from "./world-state.js";
import { appendContextMessage, clearContextStore } from "./context-store.js";
import { appendSessionEvent, clearSessionEvents, DEFAULT_EVENTS_FILE, type SessionEvent } from "./session-event.js";


// --- Types ---

export interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  savedUsd?: number;
  ttftMs: number | null;
  totalMs: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface ModelResponse {
  content: Anthropic.ContentBlock[];
  stop_reason?: string;
  usage: { input_tokens: number; output_tokens: number; cache_creation_input_tokens?: number | null; cache_read_input_tokens?: number | null };
}

export type AgentEvent =
  | { type: "text"; text: string }
  | { type: "status"; message: string }
  | { type: "user_message"; content: string }
  | { type: "api_call_start"; callNumber: number; provider: "openai" | "anthropic"; url: string; request: any }
  | { type: "llm_to_agent"; provider: "openai" | "anthropic"; url: string; stopReason: string; usage: { input_tokens: number; output_tokens: number }; content: Anthropic.ContentBlock[]; raw?: any }
  | { type: "api_error"; provider: "openai" | "anthropic"; url: string; error: string }
  | { type: "agent_to_agent_tool_call"; id: string; name: string; input: any; formatted: string }
  | { type: "agent_to_agent_tool_result"; id: string; name: string; formatted: string; result: ToolResult }
  | { type: "tool_result_message"; results: Array<{ tool_use_id: string; content: string; is_error: boolean }> }
  | { type: "world_state_saved"; path: string; charCount: number }
  | { type: "metrics"; metrics: TurnMetrics; startedAt: string }
  | { type: "turn_end"; metrics: TurnMetrics; toolCalls: string[]; provider: ProviderName; model: string }
  | { type: "error"; error: string }
  | { type: "interrupted" };

export type ProviderName = "anthropic" | "openai";

// --- Auto-approve logic ---

/** Always returns true — everything is auto-approved. No allowlist. */
export function isAutoApproved(_toolName: string, _toolInput: any): boolean {
  return true;
}

// --- Pricing ---

export const PRICING: Record<string, { input: number; output: number }> = {
  "claude-opus-4-6": { input: 5, output: 25 },
  "claude-sonnet-4-6": { input: 3, output: 15 },
  "claude-sonnet-4-20250514": { input: 3, output: 15 },
  // OpenAI Codex pricing unknown here — leave 0 until configured
  "gpt-5.2-codex": { input: 1.25, output: 10 },
};

export function estimateCost(
  model: string,
  inputTokens: number,
  outputTokens: number
): number {
  const pricing = PRICING[model] ?? { input: 5, output: 25 };
  return (
    (inputTokens * pricing.input + outputTokens * pricing.output) / 1_000_000
  );
}

/**
 * Estimate cost including Anthropic prompt cache tokens.
 * Cache write tokens are billed at 1.25× input rate.
 * Cache read tokens are billed at 0.1× input rate.
 */
export function estimateCostWithCache(
  model: string,
  inputTokens: number,
  outputTokens: number,
  cacheCreationTokens: number,
  cacheReadTokens: number
): number {
  const pricing = PRICING[model] ?? { input: 5, output: 25 };
  return (
    inputTokens * pricing.input +
    outputTokens * pricing.output +
    cacheCreationTokens * pricing.input * 1.25 +
    cacheReadTokens * pricing.input * 0.1
  ) / 1_000_000;
}

/**
 * Estimate how much was saved by prompt caching vs. paying full input rate.
 * Only cache reads produce net savings (they cost 0.1× instead of 1.0× input rate).
 * Cache writes cost 1.25× input rate, so they don't save on the turn they're written.
 */
export function estimateCacheSavings(
  model: string,
  cacheReadTokens: number
): number {
  const pricing = PRICING[model] ?? { input: 5, output: 25 };
  // Saving per cache-read token = (1.0 - 0.1) × input rate = 0.9 × input rate
  return (cacheReadTokens * pricing.input * 0.9) / 1_000_000;
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
  messages: Anthropic.MessageParam[]
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
  return err.message.includes("Extra usage is required for long context requests");
}

export function isRetryable(err: any): boolean {
  if (!err) return false;
  // A 429 from Claude Max meaning "prompt too long for your tier" is NOT transient.
  // Exclude it before the blanket status check so we don't retry fruitlessly.
  if (isContextTooLong(err)) return false;
  const status = err.status ?? err.statusCode;
  if (status === 429 || status === 529 || status === 500 || status === 503) return true;
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
  const msg: string = typeof err.message === "string" ? err.message : JSON.stringify(err);
  if (status === 401) {
    return msg.includes("authentication_error") || msg.includes("OAuth token has expired");
  }
  if (status === 403) {
    // Anthropic 403 body: {"type":"error","error":{"type":"permission_error","message":"OAuth token has been revoked..."}}
    return msg.includes("revoked") && (msg.includes("permission_error") || msg.includes("OAuth token"));
  }
  return false;
}

// --- Context window management ---
// Truncates conversation history to stay within the token budget.
// Always preserves the first user message (the original task) and the most
// recent N turns.

const KEEP_RECENT_TURNS = 10; // always keep the last 10 message pairs

// Check if a message contains tool_result blocks
function hasToolResult(msg: Anthropic.MessageParam): boolean {
  if (typeof msg.content === "string") return false;
  if (!Array.isArray(msg.content)) return false;
  return msg.content.some((b: any) => b.type === "tool_result");
}

function getOpenAiRetryDelayMs(err: any, attempt: number, baseMs: number, maxMs: number): number {
  const msg = typeof err?.message === "string" ? err.message : "";
  const match = msg.match(/try again in\s*(\d+(?:\.\d+)?)\s*s/i);
  if (match) {
    const seconds = Number(match[1]);
    if (!Number.isNaN(seconds)) {
      return Math.min(Math.ceil(seconds * 1000), maxMs);
    }
  }
  const jitter = Math.random() * 0.2 + 0.9; // 0.9–1.1
  const delay = baseMs * Math.pow(2, attempt) * jitter;
  return Math.min(Math.round(delay), maxMs);
}

function getAnthropicRetryDelayMs(_err: any, attempt: number, baseMs: number, maxMs: number): number {
  const jitter = Math.random() * 0.2 + 0.9; // 0.9–1.1
  const delay = baseMs * Math.pow(2, attempt) * jitter;
  return Math.min(Math.round(delay), maxMs);
}

// Check if a message contains tool_use blocks
function hasToolUse(msg: Anthropic.MessageParam): boolean {
  if (typeof msg.content === "string") return false;
  if (!Array.isArray(msg.content)) return false;
  return msg.content.some((b: any) => b.type === "tool_use");
}

// Get tool_use IDs from a message
function getToolUseIds(msg: Anthropic.MessageParam): Set<string> {
  const ids = new Set<string>();
  if (typeof msg.content === "string" || !Array.isArray(msg.content)) return ids;
  for (const b of msg.content) {
    if ((b as any).type === "tool_use") ids.add((b as any).id);
  }
  return ids;
}

// Get tool_result tool_use_ids from a message
function getToolResultIds(msg: Anthropic.MessageParam): Set<string> {
  const ids = new Set<string>();
  if (typeof msg.content === "string" || !Array.isArray(msg.content)) return ids;
  for (const b of msg.content) {
    if ((b as any).type === "tool_result") ids.add((b as any).tool_use_id);
  }
  return ids;
}

export function truncateHistory(
  history: Anthropic.MessageParam[],
  budget: number = config.maxContextTokens
): Anthropic.MessageParam[] {
  // Rough token estimate: ~1 token per 4 characters (conservative)
  const estimateTokens = (msg: Anthropic.MessageParam): number => {
    const text = typeof msg.content === "string"
      ? msg.content
      : JSON.stringify(msg.content);
    return Math.ceil(text.length / 4);
  };

  const MAX_MESSAGES = 100;

  let working = history;
  const reasons: string[] = [];

  // Enforce message cap first (drop from middle)
  if (working.length > MAX_MESSAGES) {
    const minKeep = Math.min(working.length, KEEP_RECENT_TURNS * 2);
    const alwaysKeepHead = working.slice(0, 1);
    const alwaysKeepTail = working.slice(-minKeep);
    const middle = working.slice(1, working.length - minKeep);
    const excess = working.length - MAX_MESSAGES;
    const trimmedMiddle = middle.slice(excess);
    working = sanitizeToolPairs([...alwaysKeepHead, ...trimmedMiddle, ...alwaysKeepTail]);
    reasons.push("message_cap");
  }

  // Count total estimated tokens
  const totalTokens = working.reduce((sum, m) => sum + estimateTokens(m), 0);
  if (totalTokens <= budget) return working;

  // Always keep first message + last KEEP_RECENT_TURNS*2 messages
  const minKeep = Math.min(working.length, KEEP_RECENT_TURNS * 2);
  const alwaysKeepHead = working.slice(0, 1);
  let alwaysKeepTail = working.slice(-minKeep);

  // Middle portion eligible for dropping
  const middle = working.slice(1, working.length - minKeep);
  if (middle.length === 0) {
    // All messages are within the "always keep" tail — history is short but each
    // message is enormous. We must still reduce: drop from the oldest end of the
    // tail, keeping at minimum the very last message (the current user turn).
    // This handles the real-world case of 11 huge messages > 641k tokens.
    let kept = [...alwaysKeepTail];
    let currentTokens = kept.reduce((sum, m) => sum + estimateTokens(m), 0);
    while (currentTokens > budget && kept.length > 1) {
      const dropped = kept.shift()!;
      currentTokens -= estimateTokens(dropped);
    }
    return sanitizeToolPairs(kept);
  }

  // Drop from oldest middle messages first
  let kept = [...middle];
  let currentTokens = totalTokens;
  while (currentTokens > budget && kept.length > 0) {
    const dropped = kept.shift()!;
    currentTokens -= estimateTokens(dropped);
  }

  let result = [...alwaysKeepHead, ...kept, ...alwaysKeepTail];

  // Fix orphaned tool_result blocks: if a tool_result references a tool_use
  // that was dropped, remove the orphaned tool_result (and its partner if needed)
  result = sanitizeToolPairs(result);

  logger.info(makeLogEntry("infra", {
    event: "context_truncated",
    originalMessages: history.length,
    keptMessages: result.length,
    droppedMessages: history.length - result.length,
    estimatedTokensBefore: history.reduce((sum, m) => sum + estimateTokens(m), 0),
    estimatedTokensAfter: result.reduce((sum, m) => sum + estimateTokens(m), 0),
    reason: reasons.length ? reasons.join("+") : "token_budget",
  }));

  return result;
}

// Remove orphaned tool_result messages (where the matching tool_use was dropped).
// Also remove orphaned tool_use messages (where the matching tool_result was dropped).
function sanitizeToolPairs(messages: Anthropic.MessageParam[]): Anthropic.MessageParam[] {
  // Collect all tool_use IDs present in the messages
  const allToolUseIds = new Set<string>();
  const allToolResultIds = new Set<string>();
  for (const msg of messages) {
    for (const id of getToolUseIds(msg)) allToolUseIds.add(id);
    for (const id of getToolResultIds(msg)) allToolResultIds.add(id);
  }

  // Filter out messages that are purely orphaned tool_results or tool_uses
  return messages.filter((msg) => {
    if (hasToolResult(msg)) {
      const resultIds = getToolResultIds(msg);
      // Keep only if ALL tool_result IDs have matching tool_use
      for (const id of resultIds) {
        if (!allToolUseIds.has(id)) return false;
      }
    }
    if (hasToolUse(msg)) {
      const useIds = getToolUseIds(msg);
      // Keep only if ALL tool_use IDs have matching tool_result
      for (const id of useIds) {
        if (!allToolResultIds.has(id)) return false;
      }
    }
    return true;
  });
}

// --- Stream event processing (extracted for testability) ---

/** Process raw Anthropic stream events into AgentEvents.
 *  This is the inner loop of sendMessage, extracted so it can be tested
 *  without a real API connection. */
export function processStreamEvents(streamEvents: Iterable<any>): AgentEvent[] {
  const events: AgentEvent[] = [];
  for (const event of streamEvents) {
    if (
      event.type === "content_block_delta" &&
      event.delta.type === "text_delta"
    ) {
      events.push({ type: "text", text: event.delta.text });
    }
    if (
      event.type === "content_block_start" &&
      event.content_block?.type === "tool_use"
    ) {
      events.push({
        type: "status",
        message: `generating ${event.content_block.name} input...`,
      });
    }
  }
  return events;
}

// --- Agent ---

/**
 * A StreamProvider is a function that calls the Anthropic messages API
 * (or a mock in tests) and returns an object with an async iterator of
 * raw stream events and a finalMessage() method.
 *
 * By accepting a StreamProvider in the constructor, the Agent can be
 * tested without hitting the real API.
 */
export type StreamProvider = (params: {
  model: string;
  max_tokens: number;
  system: string | Anthropic.TextBlockParam[];
  tools: Anthropic.Tool[];
  messages: Anthropic.MessageParam[];
}) => Promise<{
  [Symbol.asyncIterator](): AsyncIterator<any>;
  finalMessage(): Promise<Anthropic.Message>;
}>;

export class Agent {
  private client: Anthropic;
  private history: Anthropic.MessageParam[] = [];
  public sessionInputTokens = 0;
  public sessionOutputTokens = 0;
  public sessionCostUsd = 0;
  public sessionCacheCreationTokens = 0;
  public sessionCacheReadTokens = 0;
  public sessionSavedUsd = 0;
  private _apiCallCount = 0;

  private authMode: "api-key" | "oauth" = "api-key";
  private provider: ProviderName = "anthropic";
  private activeModel: string = config.model;

  public readonly sessionId: string;
  private readonly retryBaseMs = Number(process.env.OMEGA_RETRY_BASE_MS ?? 1000);
  private readonly retryMaxMs = Number(process.env.OMEGA_RETRY_MAX_MS ?? 60000);
  private readonly retryMaxAttempts = Number(process.env.OMEGA_RETRY_ATTEMPTS ?? 5);

  /** World state file path. null = disabled. undefined = use project default. */
  private readonly worldStatePath: string | null | undefined;

  /** Diagnostic output directory. null = disabled (tests). undefined = use default ("diagnosis/"). */
  private readonly diagDir: string | null | undefined;

  /** Context JSONL file path. null = disabled (tests). undefined = use production default. */
  private readonly contextFile: string | null | undefined;

  /** Events JSONL file path. null = disabled (tests). undefined = use production default. */
  private readonly eventsFile: string | null | undefined;

  /** Zone 1: world state loaded at session start, injected into system prompt. */
  private worldStateContent: string | null = null;

  /** Optional injectable stream provider (used in tests). */
  private readonly streamProvider: StreamProvider | undefined;

  /** Optional injectable OpenAI caller (used in tests). */
  private readonly openAiCaller: typeof callOpenAi;

  /**
   * Production: new Agent()
   *   — uses real Anthropic client, world state at project-specific path,
   *     context appended to sessions/context.jsonl
   * Test: new Agent(mockProvider, null, undefined, worldStatePath)
   *   — uses mock provider; world state, diagnostics, and context file are
   *     all disabled unless an explicit path is given.
   *
   * The sessionDir parameter is removed. Session persistence no longer exists.
   * The second parameter (formerly sessionDir) is kept as a positional placeholder
   * accepting null so existing test call-sites still compile.
   */
  constructor(
    streamProvider?: StreamProvider,
    _sessionDir?: string | null,
    openAiCaller: typeof callOpenAi = callOpenAi,
    worldStatePath?: string | null,
    diagDir?: string | null,
    contextFile?: string | null,
    eventsFile?: string | null
  ) {
    // Will be initialized in init()
    this.client = new Anthropic();
    this.sessionId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    this.streamProvider = streamProvider;
    this.openAiCaller = openAiCaller;
    // World state: if mock provider given and worldStatePath not specified, disable.
    if (streamProvider !== undefined && worldStatePath === undefined) {
      this.worldStatePath = null;
    } else {
      this.worldStatePath = worldStatePath;
    }
    // Diagnostics: if mock provider given and diagDir not specified, disable.
    if (streamProvider !== undefined && diagDir === undefined) {
      this.diagDir = null;
    } else {
      this.diagDir = diagDir;
    }
    // Context file: if mock provider given and contextFile not specified, disable.
    if (streamProvider !== undefined && contextFile === undefined) {
      this.contextFile = null;
    } else {
      this.contextFile = contextFile;
    }
    // Events file: if mock provider given and eventsFile not specified, disable.
    if (streamProvider !== undefined && eventsFile === undefined) {
      this.eventsFile = null;
    } else {
      this.eventsFile = eventsFile;
    }
  }

  /** Resolve the events file path (null = disabled). */
  private resolveEventsFile(): string | null {
    return this.eventsFile === undefined ? DEFAULT_EVENTS_FILE : this.eventsFile;
  }

  /** Fire-and-forget append of a SessionEvent. Errors silently dropped. */
  private logEvent(event: SessionEvent): void {
    const path = this.resolveEventsFile();
    if (path === null) return;
    appendSessionEvent(event, path).catch(() => {});
  }

  async init(): Promise<string> {
    // Auth flow (matching pi-ai's anthropic.js):
    // OAuth via claude.ai → access_token (sk-ant-oat-...)
    // Pass as authToken (Bearer auth) with Claude Code identity headers.
    // The API requires claude-code-20250219 + oauth-2025-04-20 betas.
    const accessToken = await getAuthToken();
    if (accessToken) {
      this.client = new Anthropic({
        apiKey: null as any,
        authToken: accessToken,
        defaultHeaders: {
          "accept": "application/json",
          "anthropic-dangerous-direct-browser-access": "true",
          "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
          "user-agent": "claude-cli/2.1.2 (external, cli)",
          "x-app": "cli",
        },
      });
      this.authMode = "oauth";
      startup({ authMode: "claude-max", model: config.model });
      this.logEvent({ type: "session_start", ts: new Date().toISOString(), sessionId: this.sessionId, model: this.activeModel, provider: this.provider });
      return "Claude Max";
    } else if (process.env.ANTHROPIC_API_KEY) {
      this.client = new Anthropic();
      this.authMode = "api-key";
      startup({ authMode: "api-key", model: config.model });
      this.logEvent({ type: "session_start", ts: new Date().toISOString(), sessionId: this.sessionId, model: this.activeModel, provider: this.provider });
      return "api-key (pay-per-token ⚠)";
    } else {
      throw new Error(
        "No authentication found. Run `bun run src/login.ts` to authenticate with Claude Max, or set ANTHROPIC_API_KEY."
      );
    }
  }

  setProvider(provider: ProviderName): void {
    this.provider = provider;
  }

  getAuthMode(): string {
    return this.authMode;
  }

  getProvider(): ProviderName {
    return this.provider;
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
        "accept": "application/json",
        "anthropic-dangerous-direct-browser-access": "true",
        "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
        "user-agent": "claude-cli/2.1.2 (external, cli)",
        "x-app": "cli",
      },
    });
    logger.info("oauth_reauthed", { hint: "token refreshed after 401" });
    return true;
  }

  getHistory(): readonly Anthropic.MessageParam[] {
    return this.history;
  }

  /**
   * Get a StreamProvider wrapping the real Anthropic client (or the injected
   * mock, in tests). Used for compaction LLM calls.
   */
  private getStreamProvider(): StreamProvider {
    if (this.streamProvider) return this.streamProvider;
    const client = this.client;
    return async (params) => client.messages.stream(params as any);
  }

  /**
   * Get a StreamProvider for the currently active provider.
   * If OpenAI is active, wraps `callOpenAi` to match the StreamProvider interface.
   * If Anthropic is active, returns the standard Anthropic stream provider.
   */
  private getActiveFoldProvider(): { provider: StreamProvider; providerName: ProviderName; url: string } {
    if (this.provider === "openai") {
      const openAiCaller = this.openAiCaller;
      const model = config.fallbackModel as string;
      const url = getOpenAiUrl();
      const openAiProvider: StreamProvider = async (_params) => {
        // compactWorldState always sends a single user message — extract it
        const messages = _params.messages as any[];
        const lastUserMsg = messages[messages.length - 1];
        const prompt = typeof lastUserMsg?.content === "string"
          ? lastUserMsg.content
          : JSON.stringify(lastUserMsg?.content ?? "");
        const result = await openAiCaller(
          [{ role: "user", content: prompt }],
          _params.system,
          model,
          _params.max_tokens,
          undefined
        );
        const responseText = result.text;
        const usage = result.response.usage;
        // Return a StreamProvider-compatible object
        return {
          async *[Symbol.asyncIterator]() {
            yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: responseText } };
          },
          async finalMessage() {
            return {
              id: "openai-fold",
              type: "message",
              role: "assistant",
              content: [{ type: "text", text: responseText }],
              model,
              stop_reason: "end_turn",
              stop_sequence: null,
              usage: { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens },
            } as any;
          },
        };
      };
      return { provider: openAiProvider, providerName: "openai", url };
    }
    return {
      provider: this.getStreamProvider(),
      providerName: "anthropic",
      url: "https://api.anthropic.com/v1/messages",
    };
  }

  /**
   * Fold the current session history into the world state file.
   * Call this on clean shutdown (SIGINT, SIGTERM, normal exit).
   * No-op (yields nothing) if worldStatePath is null or history is empty.
   *
   * Yields structured AgentEvents so the UI can render the LLM call and
   * file write the same way it renders regular turns.
   */
  async *foldCurrentSessionIntoWorldState(): AsyncGenerator<AgentEvent> {
    const path = this.worldStatePath === undefined ? projectWorldStatePath() : this.worldStatePath;
    if (path === null) return;
    if (this.history.length === 0) return;
    try {
      const prior = await readWorldState(path);
      const { provider, providerName, url: compactionUrl } = this.getActiveFoldProvider();

      // Build the request params so we can show them in api_call_start
      const compactionRequest = {
        model: providerName === "openai" ? (config.fallbackModel as string) : this.activeModel,
        max_tokens: 4096,
        system: "You are a context compactor. Respond only with the requested summary, no preamble.",
        tools: [],
        messages: [{ role: "user", content: "<world-state compaction prompt>" }],
      };

      yield {
        type: "api_call_start",
        callNumber: 1,
        provider: providerName,
        url: compactionUrl,
        request: compactionRequest,
      } as AgentEvent;

      // Call compactWorldState — it makes the real LLM call internally.
      // We capture usage by wrapping the provider to intercept finalMessage.
      let capturedUsage: { input_tokens: number; output_tokens: number } | null = null;
      let capturedContent: any[] = [];
      const wrapProvider = (p: StreamProvider): StreamProvider => async (params) => {
        const stream = await p(params);
        return {
          [Symbol.asyncIterator]: () => stream[Symbol.asyncIterator](),
          async finalMessage() {
            const msg = await stream.finalMessage();
            capturedUsage = msg.usage;
            capturedContent = msg.content;
            return msg;
          },
        };
      };
      const wrappedProvider = wrapProvider(provider);

      // Attempt compaction with retries for transient errors and one re-auth retry on 401/403 auth errors (Anthropic only)
      let newState: string;
      try {
        newState = await compactWorldState(prior, this.history, wrappedProvider, this.activeModel);
      } catch (compactErr: any) {
        if (isContextTooLong(compactErr)) {
          // Claude Max OAuth returns 429 "Extra usage is required for long context requests"
          // when the session history is too large for the account tier. Retrying is futile.
          // Yield a clear, actionable message and return without writing a diagnostic
          // (this is a known limitation, not a bug).
          logger.warn("world_state_fold_context_too_long", { hint: "session history too large; use /compact to reduce" });
          yield {
            type: "error",
            error: "World state fold skipped: context too long for this account tier. Use /compact to reduce history before quitting.",
          } as AgentEvent;
          return;
        } else if (isAuthExpired(compactErr) && providerName === "anthropic") {
          logger.warn("oauth_token_expired_at_fold", { hint: "attempting refresh", status: compactErr.status ?? compactErr.statusCode });
          const reauthed = await this.reinitAuth();
          if (!reauthed) {
            logger.warn("world_state_update_failed", { error: compactErr.message });
            yield { type: "error", error: `World state save failed (auth refresh failed): ${compactErr.message}` } as AgentEvent;
            return;
          }
          // Rebuild wrappedProvider with the fresh client
          const { provider: freshProvider } = this.getActiveFoldProvider();
          capturedUsage = null;
          capturedContent = [];
          const retryWrapped = wrapProvider(freshProvider);
          newState = await compactWorldState(prior, this.history, retryWrapped, this.activeModel);
        } else if (isRetryable(compactErr)) {
          // Transient stream error (e.g. "Unexpected event order" from the Anthropic SDK
          // when the server restarts a stream mid-flight) — retry once with a fresh provider.
          logger.warn("world_state_fold_transient_error", { error: compactErr.message, hint: "retrying once" });
          const { provider: freshProvider } = this.getActiveFoldProvider();
          capturedUsage = null;
          capturedContent = [];
          const retryWrapped = wrapProvider(freshProvider);
          newState = await compactWorldState(prior, this.history, retryWrapped, this.activeModel);
        } else {
          throw compactErr;
        }
      }

      yield {
        type: "llm_to_agent",
        provider: providerName,
        url: compactionUrl,
        stopReason: "end_turn",
        usage: capturedUsage ?? { input_tokens: 0, output_tokens: 0 },
        content: capturedContent,
      } as AgentEvent;

      await writeWorldState(newState, path);
      logger.info("world_state_updated", { path });

      this.logEvent({ type: "world_state_saved", ts: new Date().toISOString(), path, charCount: newState.length });
      yield {
        type: "world_state_saved",
        path,
        charCount: newState.length,
      } as AgentEvent;

    } catch (err: any) {
      logger.warn("world_state_update_failed", { error: err.message });
      // Flush log first so the snapshot's logFile pointer is current on disk.
      flushLog();
      // Write a diagnostic snapshot so the next session has full context about
      // what went wrong during shutdown.
      await writeDiagnostic(
        {
          summary: `World-state fold failed on shutdown: ${err.message}`,
          errorMessage: err.message ?? String(err),
          httpStatus: err.status ?? err.statusCode,
          provider: this.provider,
          model: this.activeModel,
          requestMessages: [],
          history: this.history,
          extra: { phase: "fold_shutdown" },
        },
        this.diagDir,
      );
      yield { type: "error", error: err.message } as AgentEvent;
    }
  }

  /**
   * Load world state from disk into memory so it can be injected into the
   * system prompt. Call once at session start, after init().
   */
  async loadWorldState(): Promise<void> {
    const path = this.worldStatePath === undefined ? projectWorldStatePath() : this.worldStatePath;
    if (path === null) return;
    try {
      this.worldStateContent = await readWorldState(path);
    } catch {
      this.worldStateContent = null;
    }
  }

  async *sendMessage(
    userMessage: string,
    _confirmTool: (name: string, input: any, formatted: string) => Promise<boolean>,
    signal?: AbortSignal
  ): AsyncGenerator<AgentEvent> {
    if (userMessage.startsWith("/")) {
      const cmd = userMessage.trim().toLowerCase();
      if (cmd === "/sonnet") {
        this.provider = "anthropic";
        this.activeModel = "claude-sonnet-4-6";
        yield { type: "status", message: "Switched to Anthropic claude-sonnet-4-6" };
      } else if (cmd === "/opus") {
        this.provider = "anthropic";
        this.activeModel = "claude-opus-4-6";
        yield { type: "status", message: "Switched to Anthropic claude-opus-4-6" };
      } else if (cmd === "/codex") {
        this.provider = "openai";
        this.activeModel = config.fallbackModel as string;
        yield { type: "status", message: `Switched to OpenAI codex (${this.activeModel})` };
      } else if (cmd === "/compact") {
        if (this.history.length === 0) {
          yield { type: "status", message: "Nothing to compact — history is empty." };
          return;
        }
        yield { type: "status", message: "Compacting context…" };
        try {
          const provider = this.getStreamProvider();
          const { history: newHistory, originalCount, newCount } = await compactHistory(
            this.history,
            provider,
            this.activeModel,
          );
          if (newCount === originalCount) {
            yield { type: "status", message: `Context is already short (${originalCount} messages) — nothing compacted.` };
          } else {
            this.history = newHistory as Anthropic.MessageParam[];
            // Rewrite context file to match the new shorter history.
            if (this.contextFile !== null) {
              await clearContextStore(this.contextFile ?? undefined, { rotate: false });
              for (const msg of this.history) {
                await appendContextMessage(msg, this.contextFile ?? undefined);
              }
            }
            this.logEvent({ type: "session_compacted", ts: new Date().toISOString(), originalCount, newCount });
            yield {
              type: "status",
              message: `Context compacted: ${originalCount} → ${newCount} messages`,
            };
          }
        } catch (err: any) {
          yield { type: "error", error: `Compaction failed: ${err.message}` };
        }
        return;
      } else if (cmd === "/help") {
        const isOpenAi = this.provider === "openai";
        const footerLegend = isOpenAi
          ? [
              "",
              "Footer:  new: <non-cached input tokens>  out: <output tokens>  cost: <=<ceiling>",
            ]
          : [
              "",
              "Footer:  new: <non-cached input, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>",
              "         cost: <actual>  saved: <cache savings>",
            ];
        yield {
          type: "status",
          message: [
            "/sonnet  — Anthropic claude-sonnet-4-6 (default)",
            "/opus    — Anthropic claude-opus-4-6",
            "/codex   — OpenAI Codex (gpt-5.2-codex)",
            "/compact — collapse history head into a summary (frees context)",
            "/help    — show this help",
            ...footerLegend,
          ].join("\n"),
        };
      } else {
        yield { type: "error", error: `Unknown command: ${userMessage}` };
      }
      return;
    }

    this.history.push({ role: "user", content: userMessage });
    if (this.contextFile !== null) {
      appendContextMessage({ role: "user", content: userMessage }, this.contextFile ?? undefined).catch(() => {}); // fire-and-forget
    }
    this.logEvent({ type: "user_message", ts: new Date().toISOString(), content: userMessage });

    // Emit user message event for UI display
    yield { type: "user_message", content: userMessage };

    logger.debug(makeLogEntry("message", {
      sender: "user",
      receiver: "agent",
      message: "call",
      contentLength: userMessage.length,
    }));

    // Reset API call counter — numbered per user prompt, not per session
    this._apiCallCount = 0;

    // Apply context window truncation before each API call
    this.history = truncateHistory(this.history) as Anthropic.MessageParam[];

    // Cumulative totals across all API calls in this user turn
    let totalInputTokens = 0;
    let totalOutputTokens = 0;
    let totalCostUsd = 0;
    let totalSavedUsd = 0;
    let totalCacheCreationTokens = 0;
    let totalCacheReadTokens = 0;
    let totalTtftMs: number | null = null;
    const allToolCalls: string[] = [];

    const fallbackEnabled = Boolean(config.fallbackModel && process.env.OPENAI_API_KEY);

    // Agentic loop: keep going while the model wants to use tools
    let continueLoop = true;
    let activeModel = this.activeModel;
    while (continueLoop) {
      continueLoop = false;

      const startTime = performance.now();
      const startedAt = new Date().toLocaleTimeString("en-GB"); // HH:MM:SS
      let ttftMs: number | null = null;
      let turnInputTokens = 0;
      let turnOutputTokens = 0;
      const toolCallsThisTurn: string[] = [];

      // Signal the UI that we're about to call the API
      yield { type: "status", message: "thinking..." } as AgentEvent;

      // For OAuth, system prompt must start with Claude Code identity
      const basePrompt = this.authMode === "oauth"
        ? "You are Claude Code, Anthropic's official CLI for Claude.\n\n" + config.systemPrompt
        : config.systemPrompt;
      // Inject zone 1 world state if available
      const systemPrompt = this.worldStateContent
        ? basePrompt + "\n\n## World State (from previous sessions)\n\n" + this.worldStateContent
        : basePrompt;

      if (this.provider === "openai" && !fallbackEnabled) {
        yield { type: "error", error: "OpenAI provider selected but OPENAI_API_KEY is not set" };
        return;
      }

      const useOpenAi = this.provider === "openai";
      activeModel = this.activeModel;

      if (useOpenAi) {
        yield {
          type: "status",
          message: `OpenAI provider active — using ${activeModel}`,
        } as AgentEvent;
      }

      // Build cached system blocks and cached tools for Anthropic prompt caching.
      // The system prompt is split into blocks with cache_control on the last block,
      // so the entire system prompt (including world state) is cached after the first call.
      // The last tool definition also gets cache_control to cache all tool definitions.
      const systemBlocks: Anthropic.TextBlockParam[] = [
        {
          type: "text",
          text: systemPrompt,
          cache_control: { type: "ephemeral" },
        },
      ];
      const cachedTools: Anthropic.Tool[] = toolDefinitions.length > 0
        ? [
            ...toolDefinitions.slice(0, -1),
            {
              ...(toolDefinitions[toolDefinitions.length - 1] as any),
              cache_control: { type: "ephemeral" },
            },
          ]
        : toolDefinitions;

      // Build cached messages: add cache_control to the last content block of
      // the last message so the entire conversation prefix is cached. This is the
      // third breakpoint (after system and tools). Opus 4.6 requires ≥4096 prefix
      // tokens for caching to activate; the extra breakpoint on messages ensures
      // the growing conversation eventually crosses that threshold.
      const cachedMessages = addCacheControlToLastMessage(this.history);

      // Emit api_call_start with a snapshot of the params before each call
      this._apiCallCount += 1;
      if (useOpenAi) {
        const openAiRequest = buildOpenAiRequest(
          this.history,
          systemPrompt,
          activeModel,
          config.maxOutputTokens
        );
        yield {
          type: "api_call_start",
          callNumber: this._apiCallCount,
          provider: "openai",
          url: getOpenAiUrl(),
          request: openAiRequest,
        } as AgentEvent;
        this.logEvent({ type: "api_call_start", ts: new Date().toISOString(), callNumber: this._apiCallCount, provider: "openai", url: getOpenAiUrl(), model: activeModel, messageCount: this.history.length });
        logger.debug(makeLogEntry("message", {
          sender: "agent",
          receiver: "llm",
          message: "call",
          callNumber: this._apiCallCount,
          provider: "openai",
          model: activeModel,
          messageCount: this.history.length,
        }));
      } else {
        const request = {
          model: activeModel,
          max_tokens: config.maxOutputTokens,
          system: systemBlocks,
          tools: cachedTools,
          messages: [...cachedMessages],
        };
        yield {
          type: "api_call_start",
          callNumber: this._apiCallCount,
          provider: "anthropic",
          url: "https://api.anthropic.com/v1/messages",
          request,
        } as AgentEvent;
        this.logEvent({ type: "api_call_start", ts: new Date().toISOString(), callNumber: this._apiCallCount, provider: "anthropic", url: "https://api.anthropic.com/v1/messages", model: activeModel, messageCount: cachedMessages.length });
        logger.debug(makeLogEntry("message", {
          sender: "agent",
          receiver: "llm",
          message: "call",
          callNumber: this._apiCallCount,
          provider: "anthropic",
          model: activeModel,
          messageCount: cachedMessages.length,
        }));
      }

      // Call API with retry
      let response: ModelResponse | null = null;
      let lastError: any = null;

      if (useOpenAi) {
        for (let attempt = 0; attempt < this.retryMaxAttempts; attempt++) {
          try {
            const openai = await this.openAiCaller(this.history, systemPrompt, activeModel, config.maxOutputTokens, signal);
            if (ttftMs === null) {
              ttftMs = performance.now() - startTime;
            }
            if (openai.text) {
              yield { type: "text", text: openai.text };
            }
            response = openai.response as any;
            (response as any).raw = openai.raw;
            lastError = null;
            break;
          } catch (err: any) {
            lastError = err;
            if (isRetryable(err) && attempt < this.retryMaxAttempts - 1) {
              const waitMs = getOpenAiRetryDelayMs(err, attempt, this.retryBaseMs, this.retryMaxMs);
              logger.warn(makeLogEntry("infra", {
                event: "api_retry",
                attempt: attempt + 1,
                provider: "openai",
                status: err.status ?? err.statusCode,
                waitMs,
                error: err.message,
              }));
              yield {
                type: "error",
                error: `${err.message ?? err}. Retrying in ${Math.round(waitMs / 1000)}s... (${attempt + 1}/${this.retryMaxAttempts})`,
              };
              await sleep(waitMs, signal);
              continue;
            }
            logger.error("api_openai_error", { error: err.message });
            flushLog();
            const diagPath = await writeDiagnostic(
              {
                summary: `OpenAI API error (status ${err.status ?? "unknown"}): ${err.message}`,
                errorMessage: err.message ?? String(err),
                httpStatus: err.status ?? err.statusCode,
                provider: "openai",
                model: activeModel,
                callNumber: this._apiCallCount,
                requestMessages: buildOpenAiRequest(this.history, systemPrompt, activeModel, config.maxOutputTokens),
                history: this.history,
                extra: { attempts: attempt + 1 },
              },
              this.diagDir,
            );
            if (diagPath) {
              logger.warn(makeLogEntry("infra", { event: "diagnostic_written", path: diagPath }));
            }
            yield {
              type: "api_error",
              provider: "openai",
              url: getOpenAiUrl(),
              error: err.message ?? String(err),
            } as AgentEvent;
            this.logEvent({ type: "api_error", ts: new Date().toISOString(), provider: "openai", url: getOpenAiUrl(), error: err.message ?? String(err), httpStatus: err.status ?? err.statusCode });
            this.logEvent({ type: "error", ts: new Date().toISOString(), error: "OpenAI rate limit. Try /sonnet or /opus to switch providers." });
            yield { type: "error", error: "OpenAI rate limit. Try /sonnet or /opus to switch providers." };
            return;
          }
        }
      } else {
        for (let attempt = 0; attempt < this.retryMaxAttempts; attempt++) {
          try {
            let fullText = "";

          const streamParams = {
            model: activeModel,
            max_tokens: config.maxOutputTokens,
            system: systemBlocks,
            tools: cachedTools,
            messages: cachedMessages,
          };
          const stream = this.streamProvider
            ? await this.streamProvider(streamParams)
            : this.client.messages.stream(streamParams);

          let aborted = false;
          for await (const event of stream) {
            if (signal?.aborted) {
              aborted = true;
              break;
            }
            if (
              event.type === "content_block_delta" &&
              event.delta.type === "text_delta"
            ) {
              if (ttftMs === null) {
                ttftMs = performance.now() - startTime;
              }
              fullText += event.delta.text;
              yield { type: "text", text: event.delta.text };
            }
            // Emit status when a tool_use block starts generating,
            // so the UI shows feedback instead of appearing stuck
            if (
              event.type === "content_block_start" &&
              event.content_block?.type === "tool_use"
            ) {
              yield {
                type: "status",
                message: `generating ${event.content_block.name} input...`,
              } as AgentEvent;
            }
          }

          if (aborted) {
            // Don't add the partial assistant turn to history.
            // The user message stays — it was real input.
            this.logEvent({ type: "interrupted", ts: new Date().toISOString() });
            yield { type: "interrupted" };
            return;
          }

          response = await stream.finalMessage();
          lastError = null;
          break;
        } catch (err: any) {
          lastError = err;

          if (isAuthExpired(err) && attempt === 0) {
            // OAuth token expired or revoked mid-session — try to refresh and retry once
            logger.warn("oauth_token_expired", { attempt: attempt + 1, status: err.status ?? err.statusCode });
            yield {
              type: "status",
              message: "OAuth token expired/revoked — refreshing...",
            } as AgentEvent;
            const reauthed = await this.reinitAuth();
            if (reauthed) {
              yield { type: "status", message: "Token refreshed, retrying..." } as AgentEvent;
              // Loop continues — the next iteration will use the fresh client
            } else {
              logger.error("oauth_reauth_failed", { error: err.message });
              yield {
                type: "api_error",
                provider: "anthropic",
                url: "https://api.anthropic.com/v1/messages",
                error: err.message ?? String(err),
              } as AgentEvent;
              this.logEvent({ type: "api_error", ts: new Date().toISOString(), provider: "anthropic", url: "https://api.anthropic.com/v1/messages", error: err.message ?? String(err), httpStatus: err.status ?? err.statusCode });
              this.logEvent({ type: "error", ts: new Date().toISOString(), error: "OAuth token expired and refresh failed." });
              yield { type: "error", error: "OAuth token expired and refresh failed. Run `bun run src/login.ts` to re-authenticate." };
              return;
            }
          } else if (isRetryable(err) && attempt < this.retryMaxAttempts - 1) {
            const waitMs = getAnthropicRetryDelayMs(err, attempt, this.retryBaseMs, this.retryMaxMs);
            logger.warn(makeLogEntry("infra", {
              event: "api_retry",
              attempt: attempt + 1,
              provider: "anthropic",
              status: err.status ?? err.statusCode,
              waitMs,
              error: err.message,
            }));
            yield {
              type: "error",
              error: `${err.message ?? err}. Retrying in ${Math.round(waitMs / 1000)}s... (${attempt + 1}/${this.retryMaxAttempts})`,
            };
            await sleep(waitMs, signal);
          } else if (
            (err.status === 400 &&
              typeof err.message === "string" &&
              err.message.includes("prompt is too long")) ||
            isContextTooLong(err)
          ) {
            // Prompt too long — aggressively truncate and retry.
            // Two cases:
            //   - 400 "prompt is too long" (standard Anthropic API key endpoint)
            //   - 429 "Extra usage is required for long context requests"
            //     (Claude Max OAuth endpoint — same root cause, different HTTP status)
            logger.warn("prompt_too_long", {
              attempt: attempt + 1,
              error: err.message,
              historyLength: this.history.length,
            });
            // Write a diagnostic so the next session has hard data on what
            // caused the overflow (exact history length, cached messages, etc.)
            flushLog();
            const diagPath = await writeDiagnostic(
              {
                summary: `Prompt too long (attempt ${attempt + 1}): ${err.message}`,
                errorMessage: err.message ?? String(err),
                httpStatus: err.status ?? err.statusCode,
                provider: "anthropic",
                model: activeModel,
                callNumber: this._apiCallCount,
                requestMessages: cachedMessages,
                systemBlocks,
                history: this.history,
                extra: { attempts: attempt + 1, stopReason: "prompt_too_long" },
              },
              this.diagDir,
            );
            if (diagPath) {
              logger.warn(makeLogEntry("infra", { event: "diagnostic_written", path: diagPath }));
            }
            // Halve the budget each retry to force more aggressive truncation
            const aggressiveBudget = Math.floor(config.maxContextTokens / (2 ** (attempt + 1)));
            this.history = truncateHistory(this.history, aggressiveBudget) as Anthropic.MessageParam[];
            yield {
              type: "error",
              error: `Prompt too long. Truncating context and retrying... (${attempt + 1}/${this.retryMaxAttempts})`,
            };
          } else {
            logger.error("api_error", { error: err.message, attempts: attempt + 1 });
            // Write a diagnostic snapshot for non-retryable errors so the next
            // session has hard data (exact request + history) to anchor debugging.
            flushLog();
            const diagPath = await writeDiagnostic(
              {
                summary: `Anthropic API error (status ${err.status ?? "unknown"}): ${err.message}`,
                errorMessage: err.message ?? String(err),
                httpStatus: err.status ?? err.statusCode,
                provider: "anthropic",
                model: activeModel,
                callNumber: this._apiCallCount,
                requestMessages: cachedMessages,
                systemBlocks,
                history: this.history,
                extra: { attempts: attempt + 1, stopReason: "api_error" },
              },
              this.diagDir,
            );
            if (diagPath) {
              logger.warn(makeLogEntry("infra", { event: "diagnostic_written", path: diagPath }));
            }
            yield {
              type: "api_error",
              provider: "anthropic",
              url: "https://api.anthropic.com/v1/messages",
              error: err.message ?? String(err),
            } as AgentEvent;
            this.logEvent({ type: "api_error", ts: new Date().toISOString(), provider: "anthropic", url: "https://api.anthropic.com/v1/messages", error: err.message ?? String(err), httpStatus: err.status ?? err.statusCode });
            if (isRetryable(err)) {
              this.logEvent({ type: "error", ts: new Date().toISOString(), error: "Anthropic rate limit." });
              yield { type: "error", error: "Anthropic rate limit. Try /codex to switch providers." };
            } else {
              this.logEvent({ type: "error", ts: new Date().toISOString(), error: `API error: ${err.message ?? err}` });
              yield { type: "error", error: `API error: ${err.message ?? err}` };
            }
            return;
          }
        }
        }
      }

      if (!response) {
        yield { type: "error", error: `API error after 5 retries: ${lastError?.message ?? lastError}` };
        return;
      }

      // Track tokens
      turnInputTokens = response.usage.input_tokens;
      turnOutputTokens = response.usage.output_tokens;
      const turnCacheCreation = (response.usage as any).cache_creation_input_tokens ?? 0;
      const turnCacheRead = (response.usage as any).cache_read_input_tokens ?? 0;
      this.sessionInputTokens += turnInputTokens;
      this.sessionOutputTokens += turnOutputTokens;
      this.sessionCacheCreationTokens += turnCacheCreation;
      this.sessionCacheReadTokens += turnCacheRead;
      const costUsd = estimateCostWithCache(
        activeModel,
        turnInputTokens,
        turnOutputTokens,
        turnCacheCreation,
        turnCacheRead
      );
      const savedUsd = estimateCacheSavings(activeModel, turnCacheRead);
      this.sessionCostUsd += costUsd;
      this.sessionSavedUsd += savedUsd;

      // Accumulate turn-level totals
      totalInputTokens += turnInputTokens;
      totalOutputTokens += turnOutputTokens;
      totalCostUsd += costUsd;
      totalSavedUsd += savedUsd;
      totalCacheCreationTokens += turnCacheCreation;
      totalCacheReadTokens += turnCacheRead;
      if (totalTtftMs === null) totalTtftMs = ttftMs; // first API call sets TTFT

      const totalMs = performance.now() - startTime;

      // Emit LLM response event for UI display
      yield {
        type: "llm_to_agent",
        provider: useOpenAi ? "openai" : "anthropic",
        url: useOpenAi ? getOpenAiUrl() : "https://api.anthropic.com/v1/messages",
        stopReason: response.stop_reason ?? "unknown",
        usage: {
          input_tokens: response.usage.input_tokens ?? 0,
          output_tokens: response.usage.output_tokens,
        },
        content: response.content,
        raw: useOpenAi ? (response as any).raw : undefined,
      };
      this.logEvent({
        type: "llm_response",
        ts: new Date().toISOString(),
        provider: useOpenAi ? "openai" : "anthropic",
        url: useOpenAi ? getOpenAiUrl() : "https://api.anthropic.com/v1/messages",
        stopReason: response.stop_reason ?? "unknown",
        model: activeModel,
        content: response.content,
        usage: {
          input_tokens: response.usage.input_tokens ?? 0,
          output_tokens: response.usage.output_tokens,
          cache_creation_input_tokens: (response.usage as any).cache_creation_input_tokens ?? undefined,
          cache_read_input_tokens: (response.usage as any).cache_read_input_tokens ?? undefined,
        },
      });
      logger.debug(makeLogEntry("message", {
        sender: "llm",
        receiver: "agent",
        message: "response",
        provider: useOpenAi ? "openai" : "anthropic",
        model: activeModel,
        stopReason: response.stop_reason ?? "unknown",
        inputTokens: response.usage.input_tokens ?? 0,
        outputTokens: response.usage.output_tokens,
        cacheCreationTokens: (response.usage as any).cache_creation_input_tokens ?? 0,
        cacheReadTokens: (response.usage as any).cache_read_input_tokens ?? 0,
        costUsd,
        ttftMs,
        totalMs: performance.now() - startTime,
      }));

      // Add assistant response to history
      this.history.push({ role: "assistant", content: response.content });
      if (this.contextFile !== null) {
        appendContextMessage({ role: "assistant", content: response.content }, this.contextFile ?? undefined).catch(() => {}); // fire-and-forget
      }

      // Process tool calls if any
      const toolUseBlocks = response.content.filter(
        (b): b is Anthropic.ToolUseBlock => b.type === "tool_use"
      );

      if (toolUseBlocks.length > 0 && response.stop_reason === "tool_use") {
        const toolResults: Anthropic.ToolResultBlockParam[] = [];

        // Emit all tool_call events first, then execute all tools in parallel,
        // then emit all tool_result events. This reduces wall-clock latency when
        // the model returns multiple tool_use blocks in one response.
        const formattedCalls: Array<{ toolUse: Anthropic.ToolUseBlock; formatted: string }> = [];
        for (const toolUse of toolUseBlocks) {
          const formatted = formatToolCall(toolUse.name, toolUse.input);
          formattedCalls.push({ toolUse, formatted });
          yield {
            type: "agent_to_agent_tool_call",
            id: toolUse.id,
            name: toolUse.name,
            input: toolUse.input,
            formatted,
          } as AgentEvent;
          this.logEvent({ type: "tool_call", ts: new Date().toISOString(), id: toolUse.id, name: toolUse.name, input: toolUse.input });
          logger.debug(makeLogEntry("message", {
            sender: "agent",
            receiver: "agent",
            message: "tool_call",
            id: toolUse.id,
            name: toolUse.name,
          }));
        }

        // Execute all tools concurrently
        const results = await Promise.all(
          formattedCalls.map(({ toolUse }) => executeTool(toolUse.name, toolUse.input))
        );

        for (let i = 0; i < formattedCalls.length; i++) {
          const { toolUse, formatted } = formattedCalls[i];
          const result = results[i];

          toolCallsThisTurn.push(toolUse.name);
          allToolCalls.push(toolUse.name);

          yield {
            type: "agent_to_agent_tool_result",
            id: toolUse.id,
            name: toolUse.name,
            formatted,
            result,
          } as AgentEvent;
          this.logEvent({ type: "tool_result", ts: new Date().toISOString(), id: toolUse.id, name: toolUse.name, isError: result.isError, durationMs: result.durationMs, outputLength: result.output.length });
          logger.debug(makeLogEntry("message", {
            sender: "agent",
            receiver: "agent",
            message: "tool_result",
            id: toolUse.id,
            name: toolUse.name,
            isError: result.isError,
            durationMs: result.durationMs,
          }));

          toolResults.push({
            type: "tool_result",
            tool_use_id: toolUse.id,
            content: result.output,
            is_error: result.isError,
          });
        }

        // Emit tool_result_message for UI display (the user message going back to API)
        yield {
          type: "tool_result_message",
          results: toolResults.map(r => ({
            tool_use_id: r.tool_use_id,
            content: r.content as string,
            is_error: r.is_error ?? false,
          })),
        };

        // Add tool results to history and continue the loop
        this.history.push({ role: "user", content: toolResults });
        if (this.contextFile !== null) {
          appendContextMessage({ role: "user", content: toolResults }, this.contextFile ?? undefined).catch(() => {}); // fire-and-forget
        }
        continueLoop = true;
      }

      // Emit metrics for this turn
      yield {
        type: "metrics",
        startedAt,
        metrics: {
          inputTokens: turnInputTokens,
          outputTokens: turnOutputTokens,
          costUsd,
          savedUsd,
          ttftMs,
          totalMs,
          cacheCreationTokens: turnCacheCreation,
          cacheReadTokens: turnCacheRead,
        },
      };
    }

    // Emit one turn_end after all API calls complete
    const endProvider: ProviderName = this.provider === "openai" ? "openai" : "anthropic";
    const endModel = activeModel;
    // Log per-turn aggregate as infra turn_end (info level per taxonomy)
    logger.info(makeLogEntry("infra", {
      event: "turn_end",
      provider: endProvider,
      model: endModel,
      inputTokens: totalInputTokens,
      outputTokens: totalOutputTokens,
      cacheCreationTokens: totalCacheCreationTokens,
      cacheReadTokens: totalCacheReadTokens,
      costUsd: totalCostUsd,
      savedUsd: totalSavedUsd,
      toolCalls: allToolCalls,
    }));
    const turnEndMetrics: TurnMetrics = {
      inputTokens: totalInputTokens,
      outputTokens: totalOutputTokens,
      costUsd: totalCostUsd,
      savedUsd: totalSavedUsd,
      ttftMs: totalTtftMs,
      totalMs: performance.now() - (this._apiCallCount > 0 ? 0 : 0), // wall time not tracked here
      cacheCreationTokens: totalCacheCreationTokens,
      cacheReadTokens: totalCacheReadTokens,
    };
    this.logEvent({ type: "turn_end", ts: new Date().toISOString(), provider: endProvider, model: endModel, metrics: turnEndMetrics, toolCalls: allToolCalls });
    yield {
      type: "turn_end",
      metrics: turnEndMetrics,
      toolCalls: allToolCalls,
      provider: endProvider,
      model: endModel,
    };

  }
}
