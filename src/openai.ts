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
    function: {
      name: tool.name,
      description: tool.description,
      parameters: tool.parameters,
    },
  }));
}

function toOpenAiMessages(history: Anthropic.MessageParam[], systemPrompt: string) {
  const messages: any[] = [];
  messages.push({ role: "system", content: systemPrompt });

  for (const msg of history) {
    if (typeof msg.content === "string") {
      messages.push({ role: msg.role, content: msg.content });
      continue;
    }
    if (!Array.isArray(msg.content)) {
      continue;
    }

    if (msg.role === "user") {
      // Tool results are sent as user content blocks in Anthropic
      const toolResults = msg.content.filter((b: any) => b.type === "tool_result");
      if (toolResults.length > 0) {
        for (const tr of toolResults) {
          messages.push({
            role: "tool",
            tool_call_id: tr.tool_use_id,
            content: typeof tr.content === "string" ? tr.content : JSON.stringify(tr.content),
          });
        }
        continue;
      }

      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      if (textBlocks.length > 0) {
        messages.push({ role: "user", content: textBlocks.join("\n") });
      }
      continue;
    }

    if (msg.role === "assistant") {
      const textBlocks = msg.content.filter((b: any) => b.type === "text").map((b: any) => b.text);
      const toolBlocks = msg.content.filter((b: any) => b.type === "tool_use");

      const message: any = {
        role: "assistant",
        content: textBlocks.length > 0 ? textBlocks.join("\n") : null,
      };

      if (toolBlocks.length > 0) {
        message.tool_calls = toolBlocks.map((b: any) => ({
          id: b.id,
          type: "function",
          function: {
            name: b.name,
            arguments: JSON.stringify(b.input ?? {}),
          },
        }));
      }

      messages.push(message);
    }
  }

  return messages;
}

export async function callOpenAi(
  history: Anthropic.MessageParam[],
  systemPrompt: string,
  model: string,
  maxTokens = config.maxOutputTokens
): Promise<OpenAiResult> {
  const apiKey = getOpenAiApiKey();
  const baseUrl = process.env.OPENAI_BASE_URL ?? "https://api.openai.com/v1";

  const messages = toOpenAiMessages(history, systemPrompt);
  const tools = toOpenAiTools();

  const resp = await fetch(`${baseUrl}/chat/completions`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Authorization": `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      model,
      messages,
      tools,
      tool_choice: "auto",
      max_tokens: maxTokens,
    }),
  });

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`OpenAI error (${resp.status}): ${text}`);
  }

  const data = await resp.json() as any;
  const choice = data.choices?.[0]?.message;
  const finishReason = data.choices?.[0]?.finish_reason ?? "stop";

  const toolCalls = choice?.tool_calls ?? [];
  const contentBlocks: Anthropic.ContentBlock[] = [];
  let fullText = "";

  if (choice?.content) {
    fullText = choice.content;
    contentBlocks.push({ type: "text", text: choice.content } as any);
  }

  if (Array.isArray(toolCalls) && toolCalls.length > 0) {
    for (const tc of toolCalls) {
      let input: any = {};
      try {
        input = tc.function?.arguments ? JSON.parse(tc.function.arguments) : {};
      } catch {
        input = {};
      }
      contentBlocks.push({
        type: "tool_use",
        id: tc.id,
        name: tc.function?.name,
        input,
      } as any);
    }
  }

  const stopReason = toolCalls.length > 0 ? "tool_use" : finishReason;

  return {
    response: {
      content: contentBlocks,
      stop_reason: stopReason,
      usage: {
        input_tokens: data.usage?.prompt_tokens ?? 0,
        output_tokens: data.usage?.completion_tokens ?? 0,
      },
    },
    text: fullText,
  };
}
