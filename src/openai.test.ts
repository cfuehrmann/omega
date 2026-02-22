import { describe, it, expect } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";
import { buildOpenAiRequest, parseOpenAiResponse } from "./openai.js";

function msg(role: "user" | "assistant", content: any): Anthropic.MessageParam {
  return { role, content } as Anthropic.MessageParam;
}

describe("buildOpenAiRequest", () => {
  it("uses string content for input messages (no input_text blocks)", () => {
    const history: Anthropic.MessageParam[] = [
      msg("user", "hello"),
      msg("assistant", "hi"),
    ];

    const req = buildOpenAiRequest(history, "sys", "gpt-5.2-codex", 10);
    const message = req.input.find((i: any) => i.role === "user");
    expect(typeof message.content).toBe("string");
    expect(JSON.stringify(req.input)).not.toContain("input_text");
  });

  it("maps tool_result blocks to function_call_output inputs", () => {
    const history: Anthropic.MessageParam[] = [
      msg("user", [
        { type: "tool_result", tool_use_id: "tool123", content: "ok", is_error: false },
      ]),
    ];

    const req = buildOpenAiRequest(history, "sys", "gpt-5.2-codex", 10);
    expect(req.input.some((i: any) => i.type === "function_call_output")).toBe(true);
    const out = req.input.find((i: any) => i.type === "function_call_output");
    expect(out.call_id).toBe("tool123");
    expect(out.output).toBe("ok");
  });
});

describe("parseOpenAiResponse", () => {
  it("maps function_call outputs to tool_use blocks", () => {
    const data = {
      output: [
        { type: "message", role: "assistant", content: [{ type: "output_text", text: "hi" }] },
        { type: "function_call", call_id: "c1", name: "read_file", arguments: "{}" },
      ],
      usage: { input_tokens: 5, output_tokens: 7 },
    };

    const parsed = parseOpenAiResponse(data);
    const tool = parsed.response.content.find((b) => b.type === "tool_use") as any;
    expect(tool).toBeTruthy();
    expect(tool.id).toBe("c1");
    expect(tool.name).toBe("read_file");
  });
});
