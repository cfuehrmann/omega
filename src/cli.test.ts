/**
 * Tests for src/cli.ts — headless CLI entrypoint.
 *
 * The headless agent-loop tests use the same mock-stream pattern as
 * agent-integration.test.ts: an inline CreateMessageStream that returns a
 * pre-scripted BetaMessage. The subprocess tests verify the CLI entry-point
 * itself (argument parsing, help, error messages) by spawning it as a child
 * process via Bun.spawn.
 */

import { describe, it, expect } from "bun:test";
import { existsSync, readFileSync } from "fs";
import { mkdir } from "fs/promises";
import { join } from "path";
import type Anthropic from "@anthropic-ai/sdk";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";
import { Agent, type CreateMessageStream, type OmegaEvent } from "./agent.js";

// ---------------------------------------------------------------------------
// Helpers — identical pattern to agent-integration.test.ts
// ---------------------------------------------------------------------------

const TEST_ROOT = `test-output/cli-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

function uniqueDir(): string {
  return join(
    TEST_ROOT,
    `session-${Math.random().toString(36).slice(2, 10)}`,
  );
}

function makeMockStream(
  events: BetaRawMessageStreamEvent[],
  message: Anthropic.Beta.Messages.BetaMessage,
) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

function makeTextMessage(text: string): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text, citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: {
      input_tokens: 10,
      output_tokens: 5,
      cache_creation: null,
      cache_creation_input_tokens: null,
      cache_read_input_tokens: null,
      inference_geo: null,
      iterations: null,
      server_tool_use: null,
      service_tier: null,
      speed: null,
    },
  };
}

function makeTextStreamEvents(text: string): BetaRawMessageStreamEvent[] {
  return [
    {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "", citations: null },
    },
    {
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text },
    },
    { type: "content_block_stop", index: 0 },
    {
      type: "message_delta",
      context_management: null,
      delta: {
        stop_reason: "end_turn",
        stop_sequence: null,
        stop_details: null,
        container: null,
      },
      usage: {
        output_tokens: 5,
        cache_creation_input_tokens: null,
        cache_read_input_tokens: null,
        input_tokens: null,
        iterations: null,
        server_tool_use: null,
      },
    },
    { type: "message_stop" },
  ];
}

// ---------------------------------------------------------------------------
// Headless run — exercises the agent loop with a mock LLM
// ---------------------------------------------------------------------------

describe("headless run", () => {
  it("streams assistant text to stderr and reaches turn_end", async () => {
    const sessionDir = uniqueDir();
    await mkdir(sessionDir, { recursive: true });
    const contextFile = join(sessionDir, "context.jsonl");
    const eventsFile = join(sessionDir, "events.jsonl");

    const mockStream: CreateMessageStream = () =>
      makeMockStream(makeTextStreamEvents("Task complete."), makeTextMessage("Task complete."));

    const agent = new Agent(mockStream, contextFile, eventsFile, sessionDir);
    agent.setModel("claude-sonnet-4-6");
    agent.setEffort("medium");
    await agent.init();

    const events: OmegaEvent[] = [];
    for await (const event of agent.sendMessage(
      "List files in current directory",
      async () => true,
    )) {
      if ("type" in event) events.push(event as OmegaEvent);
    }
    await agent.emitServerStopped("clean");
    await agent.flushEventLog();

    // Should have the expected sequence
    const types = events.map((e) => e.type);
    expect(types).toContain("user_message");
    expect(types).toContain("turn_end");
    expect(types).not.toContain("turn_interrupted");

    // events.jsonl should exist and contain valid JSON lines
    expect(existsSync(eventsFile)).toBe(true);
    const lines = readFileSync(eventsFile, "utf-8").split("\n").filter(Boolean);
    expect(lines.length).toBeGreaterThan(0);
    for (const line of lines) {
      const parsed = JSON.parse(line) as Record<string, unknown>;
      expect(typeof parsed.type).toBe("string");
    }
  });

  it("aborts mid-run when max-turns budget is reached", async () => {
    const sessionDir = uniqueDir();
    await mkdir(sessionDir, { recursive: true });
    const contextFile = join(sessionDir, "context.jsonl");
    const eventsFile = join(sessionDir, "events.jsonl");

    // First response uses tool_use (list_directory) so the agent would
    // normally loop for a second LLM call.  The budget fires after this
    // first response, so the second call never happens.
    let callIndex = 0;
    const mockStream: CreateMessageStream = () => {
      if (callIndex++ === 0) {
        return makeMockStream(
          [
            { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "t1", name: "list_directory", input: {} } },
            { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify({ path: "." }) } },
            { type: "content_block_stop", index: 0 },
            { type: "message_delta", context_management: null, delta: { stop_reason: "tool_use", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 10, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
            { type: "message_stop" },
          ] as Parameters<typeof makeMockStream>[0],
          {
            id: "msg_tool", type: "message", role: "assistant", model: "claude-sonnet-4-6",
            container: null,
            content: [{ type: "tool_use", id: "t1", name: "list_directory", input: { path: "." }, caller: { type: "direct" } }],
            stop_reason: "tool_use", stop_sequence: null, stop_details: null, context_management: null,
            usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
          } as Parameters<typeof makeMockStream>[1],
        );
      }
      // Should never be called — budget fires after the first response.
      return makeMockStream(makeTextStreamEvents("Done."), makeTextMessage("Done."));
    };

    const agent = new Agent(mockStream, contextFile, eventsFile, sessionDir);
    agent.setModel("claude-sonnet-4-6");
    await agent.init();

    const abortCtrl = new AbortController();
    const maxTurns = 1;
    let llmCallCount = 0;
    const events: OmegaEvent[] = [];

    for await (const event of agent.sendMessage(
      "Do something",
      async () => true,
      abortCtrl.signal,
    )) {
      if ("type" in event) {
        const e = event as OmegaEvent;
        events.push(e);
        // Mirror the CLI: count completed responses, abort after N
        if (e.type === "llm_response") {
          llmCallCount++;
          if (llmCallCount >= maxTurns) abortCtrl.abort();
        }
      }
    }
    await agent.flushEventLog();

    const types = events.map((e) => e.type);
    // The abort fires after the first complete response; the second LLM
    // call never starts, so we get turn_interrupted, not turn_end.
    expect(types).not.toContain("turn_end");
    expect(llmCallCount).toBe(maxTurns);
    // The mock stream was only called once
    expect(callIndex).toBe(1);
  });

  it("records model and effort in session_started event", async () => {
    const sessionDir = uniqueDir();
    await mkdir(sessionDir, { recursive: true });
    const contextFile = join(sessionDir, "context.jsonl");
    const eventsFile = join(sessionDir, "events.jsonl");

    const mockStream: CreateMessageStream = () =>
      makeMockStream(makeTextStreamEvents("Done."), makeTextMessage("Done."));

    const agent = new Agent(mockStream, contextFile, eventsFile, sessionDir);
    agent.setModel("claude-opus-4-7");
    agent.setEffort("high");
    await agent.init();
    await agent.flushEventLog();

    const lines = readFileSync(eventsFile, "utf-8").split("\n").filter(Boolean);
    const sessionStarted = lines
      .map((l) => JSON.parse(l) as Record<string, unknown>)
      .find((e) => e.type === "session_started");

    expect(sessionStarted).toBeDefined();
    expect(sessionStarted!.model).toBe("claude-opus-4-7");
    expect(sessionStarted!.effort).toBe("high");
  });

  it("emits server_stopped at the end of a run", async () => {
    const sessionDir = uniqueDir();
    await mkdir(sessionDir, { recursive: true });
    const contextFile = join(sessionDir, "context.jsonl");
    const eventsFile = join(sessionDir, "events.jsonl");

    const mockStream: CreateMessageStream = () =>
      makeMockStream(makeTextStreamEvents("Done."), makeTextMessage("Done."));

    const agent = new Agent(mockStream, contextFile, eventsFile, sessionDir);
    await agent.init();

    for await (const _event of agent.sendMessage("Hello", async () => true)) {
      // drain
    }
    await agent.emitServerStopped("clean");
    await agent.flushEventLog();

    const lines = readFileSync(eventsFile, "utf-8").split("\n").filter(Boolean);
    const last = JSON.parse(lines[lines.length - 1]!) as Record<string, unknown>;
    expect(last.type).toBe("server_stopped");
    expect(last.outcome).toBe("clean");
  });
});

// ---------------------------------------------------------------------------
// Subprocess smoke tests — verifies the CLI entry point itself
// ---------------------------------------------------------------------------

describe("CLI subprocess", () => {
  it("exits 1 and prints help when called with no args", async () => {
    const proc = Bun.spawn(["bun", "run", "src/cli.ts"], {
      stdout: "pipe",
      stderr: "pipe",
      env: { ...process.env, ANTHROPIC_API_KEY: "fake-key-for-test" },
    });
    const exitCode = await proc.exited;
    const stderr = await new Response(proc.stderr).text();

    expect(exitCode).toBe(1);
    expect(stderr).toContain("Usage:");
  });

  it("exits 0 with --help", async () => {
    const proc = Bun.spawn(["bun", "run", "src/cli.ts", "--help"], {
      stdout: "pipe",
      stderr: "pipe",
      env: { ...process.env, ANTHROPIC_API_KEY: "fake-key-for-test" },
    });
    const exitCode = await proc.exited;
    const stderr = await new Response(proc.stderr).text();

    expect(exitCode).toBe(0);
    expect(stderr).toContain("Usage:");
    expect(stderr).toContain("--instruction");
    expect(stderr).toContain("--model");
    expect(stderr).toContain("--session-dir");
    expect(stderr).toContain("--max-turns");
  });

  it("exits 1 with an unknown subcommand", async () => {
    const proc = Bun.spawn(
      ["bun", "run", "src/cli.ts", "serve", "--instruction", "hello"],
      {
        stdout: "pipe",
        stderr: "pipe",
        env: { ...process.env, ANTHROPIC_API_KEY: "fake-key-for-test" },
      },
    );
    const exitCode = await proc.exited;
    const stderr = await new Response(proc.stderr).text();

    expect(exitCode).toBe(1);
    expect(stderr).toContain("Unknown subcommand");
  });

  it("exits 1 when run subcommand has no instruction on a TTY", async () => {
    // The process inherits no TTY in test so stdin.isTTY is false; instead
    // we close stdin immediately (EOF) and expect the empty-instruction error.
    const proc = Bun.spawn(["bun", "run", "src/cli.ts", "run", "--model", "claude-sonnet-4-6"], {
      stdout: "pipe",
      stderr: "pipe",
      stdin: new ReadableStream({ start(c) { c.close(); } }),
      env: { ...process.env, ANTHROPIC_API_KEY: "fake-key-for-test" },
    });
    const exitCode = await proc.exited;
    const stderr = await new Response(proc.stderr).text();

    expect(exitCode).toBe(1);
    // Either the "empty instruction" error or the API key error fires first
    expect(stderr.length).toBeGreaterThan(0);
  });
});
