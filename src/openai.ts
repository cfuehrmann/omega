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
  instructions?: string;
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
    parameters: {
      ...((tool as any).input_schema ?? {}),
      additionalProperties: false,
    },
    strict: true,
  }));
}

export function buildOpenAiRequest(
  history: Anthropic.MessageParam[],
  systemPrompt: string,
  model: string,
  maxTokens: number
): OpenAiRequest {
  const input: any[] = [];
  const toolUseById = new Map<string, { name: string; input: any }>();
  const emittedToolCalls = new Set<string>();

  // Pre-scan for tool_use blocks so we can insert missing function_call entries
  for (const msg of history) {
    if (msg.role !== "assistant" || !Array.isArray(msg.content)) continue;
    for (const b of msg.content as any[]) {
      if (b.type === "tool_use") {
        toolUseById.set(b.id, { name: b.name, input: b.input ?? {} });
      }
    }
  }

  for (const msg of history) {
    if (typeof msg.content === "string") {
      input.push({ role: msg.role, content: msg.content });
      continue;
    }
    if (!Array.isArray(msg.content)) continue;

    if (msg.role === "user") {
      const toolResults = msg.content.filter((b: any) => b.type === "tool_result");
      if (toolResults.length > 0) {
        for (const tr of toolResults) {
        if (!emittedToolCalls.has(tr.tool_use_id)) {
          const toolUse = toolUseById.get(tr.tool_use_id);
          if (toolUse) {
            input.push({
              type: "function_call",
              call_id: tr.tool_use_id,
              name: toolUse.name,
              arguments: JSON.stringify(toolUse.input ?? {}),
            });
            emittedToolCalls.add(tr.tool_use_id);
          }
        }
        input.push({
          type: "function_call_output",
          call_id: tr.tool_use_id,
          output: typeof tr.content === "string" ? tr.content : JSON.stringify(tr.content),
        });
      }
      }

      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      if (textBlocks.length > 0) {
        input.push({ role: "user", content: textBlocks.join("\n") });
      }
      continue;
    }

    if (msg.role === "assistant") {
      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      if (textBlocks.length > 0) {
        input.push({ role: "assistant", content: textBlocks.join("\n") });
      }

      const toolUses = msg.content.filter((b: any) => b.type === "tool_use");
      for (const tu of toolUses) {
        input.push({
          type: "function_call",
          call_id: tu.id,
          name: tu.name,
          arguments: JSON.stringify(tu.input ?? {}),
        });
        emittedToolCalls.add(tu.id);
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
    instructions: systemPrompt,
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
