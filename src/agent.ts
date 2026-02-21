import Anthropic from "@anthropic-ai/sdk";
import { config } from "./config.js";
import { toolDefinitions, executeTool, formatToolCall, type ToolResult } from "./tools.js";
import { getAuthToken } from "./auth.js";
import { logger } from "./logger.js";

// --- Types ---

export interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null;
  totalMs: number;
}

export type AgentEvent =
  | { type: "text"; text: string }
  | { type: "status"; message: string }
  | { type: "tool_call"; id: string; name: string; input: any; formatted: string }
  | { type: "tool_pending"; id: string; name: string; formatted: string }
  | { type: "tool_result"; id: string; name: string; result: ToolResult }
  | { type: "tool_rejected"; id: string; name: string }
  | { type: "metrics"; metrics: TurnMetrics }
  | { type: "error"; error: string };

type ConfirmFn = (
  name: string,
  input: any,
  formatted: string
) => Promise<boolean>;

// --- Auto-approve logic ---

export function isAutoApproved(toolName: string, toolInput: any): boolean {
  if (config.autoApproveTools.includes(toolName)) {
    return true;
  }
  if (toolName === "run_command" && toolInput?.command) {
    const cmd = toolInput.command.trim();
    return config.autoApproveCommands.some(
      (prefix) => cmd === prefix || cmd.startsWith(prefix + " ")
    );
  }
  return false;
}

// --- Pricing ---

export const PRICING: Record<string, { input: number; output: number }> = {
  "claude-opus-4-6": { input: 15, output: 75 },
  "claude-sonnet-4-6": { input: 3, output: 15 },
  "claude-sonnet-4-20250514": { input: 3, output: 15 },
};

export function estimateCost(
  model: string,
  inputTokens: number,
  outputTokens: number
): number {
  const pricing = PRICING[model] ?? { input: 15, output: 75 };
  return (
    (inputTokens * pricing.input + outputTokens * pricing.output) / 1_000_000
  );
}

// --- Retry logic ---

async function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function isRetryable(err: any): boolean {
  if (!err) return false;
  const status = err.status ?? err.statusCode;
  return status === 429 || status === 529 || status === 500 || status === 503;
}

// --- Context window management ---
// Truncates conversation history to stay within the token budget.
// Always preserves the first user message (the original task) and the most
// recent N turns.

const KEEP_RECENT_TURNS = 10; // always keep the last 10 message pairs

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

  // Count total estimated tokens
  const totalTokens = history.reduce((sum, m) => sum + estimateTokens(m), 0);
  if (totalTokens <= budget) return history;

  // Always keep first message + last KEEP_RECENT_TURNS*2 messages
  const minKeep = Math.min(history.length, KEEP_RECENT_TURNS * 2);
  const alwaysKeepHead = history.slice(0, 1);
  const alwaysKeepTail = history.slice(-minKeep);

  // Middle portion eligible for dropping
  const middle = history.slice(1, history.length - minKeep);
  if (middle.length === 0) return history;

  // Drop from oldest middle messages first
  let kept = [...middle];
  let currentTokens = totalTokens;
  while (currentTokens > budget && kept.length > 0) {
    const dropped = kept.shift()!;
    currentTokens -= estimateTokens(dropped);
  }

  logger.info("context_truncated", {
    originalMessages: history.length,
    keptMessages: 1 + kept.length + alwaysKeepTail.length,
    droppedMessages: middle.length - kept.length,
    estimatedTokensBefore: totalTokens,
    estimatedTokensAfter: currentTokens,
  });

  return [...alwaysKeepHead, ...kept, ...alwaysKeepTail];
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

export class Agent {
  private client: Anthropic;
  private history: Anthropic.MessageParam[] = [];
  public sessionInputTokens = 0;
  public sessionOutputTokens = 0;
  public sessionCostUsd = 0;

  private authMode: "api-key" | "oauth" = "api-key";

  constructor() {
    // Will be initialized in init()
    this.client = new Anthropic();
  }

  async init(): Promise<string> {
    // Try OAuth token first (Claude Max), fall back to API key
    const oauthToken = await getAuthToken();
    if (oauthToken) {
      this.client = new Anthropic({
        authToken: oauthToken,
        apiKey: undefined as any,
      });
      this.authMode = "oauth";
      logger.startup({ authMode: "oauth", model: config.model });
      return "oauth (Claude Max)";
    } else if (process.env.ANTHROPIC_API_KEY) {
      this.client = new Anthropic();
      this.authMode = "api-key";
      logger.startup({ authMode: "api-key", model: config.model });
      return "api-key";
    } else {
      throw new Error(
        "No authentication found. Run `bun run src/login.ts` to authenticate with Claude Max, or set ANTHROPIC_API_KEY."
      );
    }
  }

  getAuthMode(): string {
    return this.authMode;
  }

  getHistory(): readonly Anthropic.MessageParam[] {
    return this.history;
  }

  async *sendMessage(
    userMessage: string,
    confirmTool: ConfirmFn
  ): AsyncGenerator<AgentEvent> {
    this.history.push({ role: "user", content: userMessage });

    // Apply context window truncation before each API call
    this.history = truncateHistory(this.history) as Anthropic.MessageParam[];

    // Agentic loop: keep going while the model wants to use tools
    let continueLoop = true;
    while (continueLoop) {
      continueLoop = false;

      const startTime = performance.now();
      let ttftMs: number | null = null;
      let turnInputTokens = 0;
      let turnOutputTokens = 0;
      const toolCallsThisTurn: string[] = [];

      // Signal the UI that we're about to call the API
      yield { type: "status", message: "thinking..." } as AgentEvent;

      // Call API with retry
      let response: Anthropic.Message | null = null;
      let lastError: any = null;
      for (let attempt = 0; attempt < 5; attempt++) {
        try {
          let fullText = "";

          const stream = this.client.messages.stream({
            model: config.model,
            max_tokens: config.maxOutputTokens,
            system: config.systemPrompt,
            tools: toolDefinitions,
            messages: this.history,
          });

          for await (const event of stream) {
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

          response = await stream.finalMessage();
          lastError = null;
          break;
        } catch (err: any) {
          lastError = err;
          if (isRetryable(err) && attempt < 4) {
            const waitMs = Math.min(1000 * Math.pow(2, attempt), 60000);
            logger.warn("api_retry", {
              attempt: attempt + 1,
              status: err.status ?? err.statusCode,
              waitMs,
              error: err.message,
            });
            yield {
              type: "error",
              error: `${err.message ?? err}. Retrying in ${Math.round(waitMs / 1000)}s... (${attempt + 1}/5)`,
            };
            await sleep(waitMs);
          } else {
            logger.error("api_error", { error: err.message, attempts: attempt + 1 });
            yield { type: "error", error: `API error: ${err.message ?? err}` };
            return;
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
      this.sessionInputTokens += turnInputTokens;
      this.sessionOutputTokens += turnOutputTokens;
      const costUsd = estimateCost(config.model, turnInputTokens, turnOutputTokens);
      this.sessionCostUsd += costUsd;

      const totalMs = performance.now() - startTime;

      // Add assistant response to history
      this.history.push({ role: "assistant", content: response.content });

      // Process tool calls if any
      const toolUseBlocks = response.content.filter(
        (b): b is Anthropic.ToolUseBlock => b.type === "tool_use"
      );

      if (toolUseBlocks.length > 0 && response.stop_reason === "tool_use") {
        const toolResults: Anthropic.ToolResultBlockParam[] = [];

        for (const toolUse of toolUseBlocks) {
          const formatted = formatToolCall(toolUse.name, toolUse.input);
          const autoApproved = isAutoApproved(toolUse.name, toolUse.input);

          let approved: boolean;
          if (autoApproved) {
            // Skip confirmation entirely — just emit tool_call directly
            approved = true;
          } else {
            // Ask for confirmation via UI
            yield {
              type: "tool_pending",
              id: toolUse.id,
              name: toolUse.name,
              formatted,
            };

            approved = await confirmTool(
              toolUse.name,
              toolUse.input,
              formatted
            );
          }

          if (approved) {
            yield {
              type: "tool_call",
              id: toolUse.id,
              name: toolUse.name,
              input: toolUse.input,
              formatted,
            };

            const result = await executeTool(toolUse.name, toolUse.input);
            toolCallsThisTurn.push(toolUse.name);

            logger.toolExec({
              name: toolUse.name,
              autoApproved,
              approved: true,
              isError: result.isError,
              durationMs: result.durationMs,
            });

            yield {
              type: "tool_result",
              id: toolUse.id,
              name: toolUse.name,
              result,
            };

            toolResults.push({
              type: "tool_result",
              tool_use_id: toolUse.id,
              content: result.output,
              is_error: result.isError,
            });
          } else {
            logger.toolExec({
              name: toolUse.name,
              autoApproved,
              approved: false,
              isError: false,
              durationMs: 0,
            });

            yield {
              type: "tool_rejected",
              id: toolUse.id,
              name: toolUse.name,
            };

            toolResults.push({
              type: "tool_result",
              tool_use_id: toolUse.id,
              content: "Tool call rejected by operator.",
              is_error: true,
            });
          }
        }

        // Add tool results to history and continue the loop
        this.history.push({ role: "user", content: toolResults });
        continueLoop = true;
      }

      // Log the API call
      logger.apiCall({
        model: config.model,
        inputTokens: turnInputTokens,
        outputTokens: turnOutputTokens,
        costUsd,
        ttftMs,
        totalMs,
        toolCalls: toolCallsThisTurn,
        stopReason: response.stop_reason ?? "unknown",
      });

      // Emit metrics for this turn
      yield {
        type: "metrics",
        metrics: {
          inputTokens: turnInputTokens,
          outputTokens: turnOutputTokens,
          costUsd,
          ttftMs,
          totalMs,
        },
      };
    }
  }
}
