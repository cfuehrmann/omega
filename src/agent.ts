import Anthropic from "@anthropic-ai/sdk";
import { config } from "./config.js";
import { toolDefinitions, executeTool, type ToolResult } from "./tools.js";

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

// --- Pricing ---

const PRICING: Record<string, { input: number; output: number }> = {
  "claude-opus-4-6": { input: 15, output: 75 },
  "claude-sonnet-4-20250514": { input: 3, output: 15 },
};

function estimateCost(
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

function isRetryable(err: any): boolean {
  if (!err) return false;
  const status = err.status ?? err.statusCode;
  return status === 429 || status === 529 || status === 500 || status === 503;
}

// --- Agent ---

export class Agent {
  private client: Anthropic;
  private history: Anthropic.MessageParam[] = [];
  public sessionInputTokens = 0;
  public sessionOutputTokens = 0;
  public sessionCostUsd = 0;

  constructor() {
    this.client = new Anthropic();
  }

  getHistory(): readonly Anthropic.MessageParam[] {
    return this.history;
  }

  async *sendMessage(
    userMessage: string,
    confirmTool: ConfirmFn
  ): AsyncGenerator<AgentEvent> {
    this.history.push({ role: "user", content: userMessage });

    // Agentic loop: keep going while the model wants to use tools
    let continueLoop = true;
    while (continueLoop) {
      continueLoop = false;

      const startTime = performance.now();
      let ttftMs: number | null = null;
      let turnInputTokens = 0;
      let turnOutputTokens = 0;

      // Call API with retry
      let response: Anthropic.Message | null = null;
      let lastError: any = null;
      for (let attempt = 0; attempt < 5; attempt++) {
        try {
          // Stream the response
          const contentBlocks: Anthropic.ContentBlock[] = [];
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
          }

          response = await stream.finalMessage();
          lastError = null;
          break;
        } catch (err: any) {
          lastError = err;
          if (isRetryable(err) && attempt < 4) {
            const waitMs = Math.min(1000 * Math.pow(2, attempt), 60000);
            yield {
              type: "error",
              error: `${err.message ?? err}. Retrying in ${Math.round(waitMs / 1000)}s... (${attempt + 1}/5)`,
            };
            await sleep(waitMs);
          } else {
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
          const { formatToolCall } = await import("./tools.js");
          const formatted = formatToolCall(toolUse.name, toolUse.input);

          // Ask for confirmation
          yield {
            type: "tool_pending",
            id: toolUse.id,
            name: toolUse.name,
            formatted,
          };

          const approved = await confirmTool(
            toolUse.name,
            toolUse.input,
            formatted
          );

          if (approved) {
            yield {
              type: "tool_call",
              id: toolUse.id,
              name: toolUse.name,
              input: toolUse.input,
              formatted,
            };

            const result = await executeTool(toolUse.name, toolUse.input);

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
