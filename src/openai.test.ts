import { describe, it, expect } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";
import { buildOpenAiRequest, parseOpenAiResponse, callOpenAi } from "./openai.js";

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

  it("maps tool_use blocks to function_call inputs", () => {
    const history: Anthropic.MessageParam[] = [
      msg("assistant", [
        { type: "tool_use", id: "tool123", name: "read_file", input: { path: "x" } },
      ]),
    ];

    const req = buildOpenAiRequest(history, "sys", "gpt-5.2-codex", 10);
    const call = req.input.find((i: any) => i.type === "function_call");
    expect(call).toBeTruthy();
    expect(call.call_id).toBe("tool123");
    expect(call.name).toBe("read_file");
  });

  it("maps tool_result blocks to function_call_output inputs", () => {
    const history: Anthropic.MessageParam[] = [
      msg("assistant", [
        { type: "tool_use", id: "tool123", name: "read_file", input: { path: "x" } },
      ]),
      msg("user", [
        { type: "tool_result", tool_use_id: "tool123", content: "ok", is_error: false },
      ]),
    ];

    const req = buildOpenAiRequest(history, "sys", "gpt-5.2-codex", 10);
    const out = req.input.find((i: any) => i.type === "function_call_output");
    expect(out).toBeTruthy();
    expect(out.call_id).toBe("tool123");
    expect(out.output).toBe("ok");
  });

  it("inserts a function_call before function_call_output when history is out of order", () => {
    const history: Anthropic.MessageParam[] = [
      msg("user", [
        { type: "tool_result", tool_use_id: "tool123", content: "ok", is_error: false },
      ]),
      msg("assistant", [
        { type: "tool_use", id: "tool123", name: "read_file", input: { path: "x" } },
      ]),
    ];

    const req = buildOpenAiRequest(history, "sys", "gpt-5.2-codex", 10);
    const callIndex = req.input.findIndex((i: any) => i.type === "function_call" && i.call_id === "tool123");
    const outIndex = req.input.findIndex((i: any) => i.type === "function_call_output" && i.call_id === "tool123");
    expect(callIndex).toBeGreaterThanOrEqual(0);
    expect(outIndex).toBeGreaterThanOrEqual(0);
    expect(callIndex).toBeLessThan(outIndex);
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

  it("marks tools as strict for OpenAI validation", () => {
    const req = buildOpenAiRequest([], "sys", "gpt-5.2-codex", 10);
    const tool = req.tools[0];
    expect(tool.strict).toBe(true);
  });

  it("sets additionalProperties=false in tool schemas", () => {
    const req = buildOpenAiRequest([], "sys", "gpt-5.2-codex", 10);
    const tool = req.tools[0];
    expect(tool.parameters.additionalProperties).toBe(false);
  });

  it("uses input_schema as parameters with type object", () => {
    const req = buildOpenAiRequest([], "sys", "gpt-5.2-codex", 10);
    const tool = req.tools.find((t: any) => t.name === "read_file");
    expect(tool.parameters.type).toBe("object");
    expect(tool.parameters.properties.path.type).toBe("string");
  });

  it("requires all properties when strict", () => {
    const req = buildOpenAiRequest([], "sys", "gpt-5.2-codex", 10);
    const tool = req.tools.find((t: any) => t.name === "read_file");
    const required = tool.parameters.required;
    expect(required).toContain("path");
    expect(required).toContain("offset");
    expect(required).toContain("limit");
  });
});

describe("callOpenAi abort", () => {
  it("rejects immediately when signal is already aborted (fetch receives signal)", async () => {
    // Patch global fetch to capture the signal passed to it
    const originalFetch = globalThis.fetch;
    let capturedSignal: AbortSignal | undefined;
    globalThis.fetch = (async (_url: any, opts: any) => {
      capturedSignal = opts?.signal;
      // Simulate a slow response that never resolves
      await new Promise((_resolve, reject) => {
        if (opts?.signal?.aborted) reject(new DOMException("Aborted", "AbortError"));
        opts?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      });
      return new Response("{}");
    }) as any;

    const controller = new AbortController();
    controller.abort(); // already aborted

    process.env.OPENAI_API_KEY = "test-key";
    try {
      await expect(callOpenAi([], "sys", "model", 10, controller.signal)).rejects.toThrow();
      expect(capturedSignal).toBeDefined();
    } finally {
      globalThis.fetch = originalFetch;
      delete process.env.OPENAI_API_KEY;
    }
  });
});
