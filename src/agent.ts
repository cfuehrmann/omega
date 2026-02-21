import Anthropic from "@anthropic-ai/sdk";
import { config } from "./config.js";

export interface Message {
  role: "user" | "assistant";
  content: string;
}

export interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null; // time to first token
  totalMs: number;
}

// Anthropic pricing per million tokens (as of 2025-02)
const PRICING: Record<string, { input: number; output: number }> = {
  "claude-opus-4-20250514": { input: 15, output: 75 },
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

export class Agent {
  private client: Anthropic;
  private history: Message[] = [];
  public sessionInputTokens = 0;
  public sessionOutputTokens = 0;
  public sessionCostUsd = 0;

  constructor() {
    this.client = new Anthropic();
  }

  getHistory(): readonly Message[] {
    return this.history;
  }

  async *sendMessage(
    userMessage: string
  ): AsyncGenerator<{ type: "text"; text: string } | { type: "metrics"; metrics: TurnMetrics }> {
    this.history.push({ role: "user", content: userMessage });

    const startTime = performance.now();
    let ttftMs: number | null = null;
    let fullText = "";

    const stream = this.client.messages.stream({
      model: config.model,
      max_tokens: config.maxOutputTokens,
      system: config.systemPrompt,
      messages: this.history.map((m) => ({
        role: m.role,
        content: m.content,
      })),
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

    const finalMessage = await stream.finalMessage();
    const totalMs = performance.now() - startTime;

    const inputTokens = finalMessage.usage.input_tokens;
    const outputTokens = finalMessage.usage.output_tokens;
    const costUsd = estimateCost(config.model, inputTokens, outputTokens);

    this.sessionInputTokens += inputTokens;
    this.sessionOutputTokens += outputTokens;
    this.sessionCostUsd += costUsd;

    this.history.push({ role: "assistant", content: fullText });

    yield {
      type: "metrics",
      metrics: { inputTokens, outputTokens, costUsd, ttftMs, totalMs },
    };
  }
}
