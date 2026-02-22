import { config } from "./config.js";
import { toolDefinitions } from "./tools.js";
import type Anthropic from "@anthropic-ai/sdk";

export interface OpenAiResult {
  response: {
    content: Anthropic.ContentBlock[];
    stop_reason: string;
    usage: { input_tokens: number; output_tokens: number };
  };
  text: string;
}

interface OpenAiRequest {
  model: string;
  input: any[];
  tools: any[];
  tool_choice: "auto";
  max_output_tokens: number;
}

function getOpenAiApiKey(): string {
  const key = process.env.OPENAI_API_KEY;
  if (!key) {
    throw new Error("OPENAI_API_KEY is not set");
  }
  return key;
}

function toOpenAiTools() {
  return toolDefinitions.map((tool) => ({
    type: "function",
    name: tool.name,
    description: tool.description,
    parameters: tool.parameters,
  }));
}

function pushTextMessage(target: any[], role: string, text: string) {
  if (!text) return;
  target.push({
    role,
    content: [{ type: "input_text", text }],
  });
}

export function buildOpenAiRequest(
  history: Anthropic.MessageParam[],
  systemPrompt: string,
  model: string,
  maxTokens: number
): OpenAiRequest {
  const input: any[] = [];

  // System prompt
  pushTextMessage(input, "system", systemPrompt);

  for (const msg of history) {
    if (typeof msg.content === "string") {
      pushTextMessage(input, msg.role, msg.content);
      continue;
    }
    if (!Array.isArray(msg.content)) continue;

    if (msg.role === "user") {
      const toolResults = msg.content.filter((b: any) => b.type === "tool_result");
      if (toolResults.length > 0) {
        for (const tr of toolResults) {
          input.push({
            type: "function_call_output",
            call_id: tr.tool_use_id,
            output: typeof tr.content === "string" ? tr.content : JSON.stringify(tr.content),
          });
        }
      }

      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      if (textBlocks.length > 0) {
        pushTextMessage(input, "user", textBlocks.join("\n"));
      }
      continue;
    }

    if (msg.role === "assistant") {
      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      if (textBlocks.length > 0) {
        pushTextMessage(input, "assistant", textBlocks.join("\n"));
      }
      continue;
    }
  }

  return {
    model,
    input,
    tools: toOpenAiTools(),
    tool_choice: "auto",
    max_output_tokens: maxTokens,
  };
}

export function parseOpenAiResponse(data: any): OpenAiResult {
  const contentBlocks: Anthropic.ContentBlock[] = [];
  let fullText = "";

  const outputs = Array.isArray(data.output) ? data.output : [];
  for (const item of outputs) {
    if (item.type === "message") {
      const parts = Array.isArray(item.content) ? item.content : [];
      for (const p of parts) {
        if (p.type === "output_text") {
          fullText += p.text;
        }
      }
      if (fullText) {
        contentBlocks.push({ type: "text", text: fullText } as any);
      }
    }

    if (item.type === "function_call") {
      let args: any = {};
      try {
        args = item.arguments ? JSON.parse(item.arguments) : {};
      } catch {
        args = {};
      }
      contentBlocks.push({
        type: "tool_use",
        id: item.call_id,
        name: item.name,
        input: args,
      } as any);
    }
  }

  const stopReason = contentBlocks.some((b) => b.type === "tool_use") ? "tool_use" : "stop";

  return {
    response: {
      content: contentBlocks,
      stop_reason: stopReason,
      usage: {
        input_tokens: data.usage?.input_tokens ?? data.usage?.prompt_tokens ?? 0,
        output_tokens: data.usage?.output_tokens ?? data.usage?.completion_tokens ?? 0,
      },
    },
    text: fullText,
  };
}

export async function callOpenAi(
  history: Anthropic.MessageParam[],
  systemPrompt: string,
  model: string,
  maxTokens = config.maxOutputTokens
): Promise<OpenAiResult> {
  const apiKey = getOpenAiApiKey();
  const baseUrl = process.env.OPENAI_BASE_URL ?? "https://api.openai.com/v1";

  const body = buildOpenAiRequest(history, systemPrompt, model, maxTokens);

  const resp = await fetch(`${baseUrl}/responses`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Authorization": `Bearer ${apiKey}`,
    },
    body: JSON.stringify(body),
  });

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`OpenAI error (${resp.status}): ${text}`);
  }

  const data = await resp.json();
  return parseOpenAiResponse(data);
}
