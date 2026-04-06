/**
 * Tests for session resumption — basis extraction and summarisation.
 *
 * Covers:
 *   - extractResumptionBasis: empty session, single turn, tool calls, errors,
 *     carry-forward from prior session_resumed, multiple turns
 *   - extractSummaryFromResponse: tag extraction and fallback
 *   - generateSessionName: calls provider, sanitises output
 *   - Agent.seedWithResumptionSummary: emits event, seeds context
 *   - Agent.performResumption: streaming integration
 */

import { describe, it, expect, afterAll } from "bun:test";
import { extractResumptionBasis, extractSummaryFromResponse, extractDescriptionFromResponse, generateSessionName, AUTO_NAME_INSTRUCTIONS } from "./session-resume.js";
import type { StreamProvider } from "./stream-provider.js";
import type { OmegaEvent } from "./events.js";
import { makeTestAgent } from "./test-utils.js";
import { OmegaEventSchema } from "./events.schema.js";
import { readFileSync, existsSync } from "fs";

// ---------------------------------------------------------------------------
// StreamProvider mock helpers
// ---------------------------------------------------------------------------

/**
 * Build a StreamProvider mock that yields a single text chunk and returns
 * the given text as the final message. Suitable for performResumption tests.
 */
function makeTextStreamProvider(text: string): StreamProvider {
  return () => ({
    async *[Symbol.asyncIterator]() {
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } };
    },
    finalMessage: async () => ({
      id: "msg_test",
      type: "message",
      role: "assistant",
      content: [{ type: "text", text }],
      model: "claude-sonnet-4-6",
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 10 },
    } as any),
  });
}

/**
 * Build a StreamProvider mock that throws when called.
 * Suitable for testing error paths in performResumption.
 */
function makeFailingStreamProvider(message: string): StreamProvider {
  return () => { throw new Error(message); };
}

/**
 * Build a StreamProvider mock for generateSessionName tests.
 * Accepts an optional callback to capture the params passed to the provider.
 */
function makeNameStreamProvider(text: string, onCall?: (params: any) => void): StreamProvider {
  return (params) => {
    onCall?.(params);
    return {
      async *[Symbol.asyncIterator]() { /* no chunks needed for naming */ },
      finalMessage: async () => ({
        id: "msg_name",
        type: "message",
        role: "assistant",
        content: [{ type: "text", text }],
        model: "claude-sonnet-4-6",
        stop_reason: "end_turn",
        stop_sequence: null,
        usage: { input_tokens: 5, output_tokens: 3 },
      } as any),
    };
  };
}

// ---------------------------------------------------------------------------
// Minimal event builders
// ---------------------------------------------------------------------------

function userMsg(content: string): OmegaEvent {
  return { type: "user_message", time: "2025-01-01T00:00:00.000Z" as any, content };
}

function llmResp(text: string, stopReason = "end_turn"): OmegaEvent {
  return {
    type: "llm_response",
    time: "2025-01-01T00:00:01.000Z" as any,
    stopReason,
    usage: { input_tokens: 10, output_tokens: 5 },
    contextHash: "abc123def456" as any,
    text,
  };
}

function toolCall(id: string, name: string, input: unknown): OmegaEvent {
  return {
    type: "tool_call",
    time: "2025-01-01T00:00:02.000Z" as any,
    id,
    name,
    input,
    contextHash: "abc123def456" as any,
  };
}

function toolResult(id: string, name: string, output: string, isError = false): OmegaEvent {
  return {
    type: "tool_result",
    time: "2025-01-01T00:00:03.000Z" as any,
    id,
    name,
    output,
    isError,
    durationMs: 100,
  };
}

function turnEnd(): OmegaEvent {
  return {
    type: "turn_end",
    time: "2025-01-01T00:00:04.000Z" as any,
    metrics: { inputTokens: 10, outputTokens: 5 },
  };
}

function turnInterrupted(reason?: "aborted" | "error"): OmegaEvent {
  return { type: "turn_interrupted", time: "2025-01-01T00:00:04.000Z" as any, reason };
}

function sessionResumed(continuationOf: string, summary: string): OmegaEvent {
  return {
    type: "session_resumed",
    time: "2025-01-01T00:00:00.000Z" as any,
    continuationOf,
    summary,
  };
}

function agentError(error: string): OmegaEvent {
  return { type: "agent_error", time: "2025-01-01T00:00:02.000Z" as any, error };
}

// ---------------------------------------------------------------------------
// extractResumptionBasis
// ---------------------------------------------------------------------------

describe("extractResumptionBasis — empty session", () => {
  it("returns empty-session message when no events", () => {
    expect(extractResumptionBasis([])).toBe(
      "(empty session — no turns recorded)",
    );
  });

  it("returns empty-session message when events have no turns", () => {
    const events: OmegaEvent[] = [
      { type: "server_started", time: "2025-01-01T00:00:00.000Z" as any },
      { type: "session_started", time: "2025-01-01T00:00:00.000Z" as any, sessionId: "x", model: "m", authMode: "api-key", systemPrompt: "s" },
    ];
    expect(extractResumptionBasis(events)).toBe(
      "(empty session — no turns recorded)",
    );
  });
});

describe("extractResumptionBasis — single turn", () => {
  it("includes user message", () => {
    const events: OmegaEvent[] = [
      userMsg("Write a hello-world function"),
      llmResp("Here is a hello-world function."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("User: Write a hello-world function");
  });

  it("includes agent text response", () => {
    const events: OmegaEvent[] = [
      userMsg("Write a hello-world function"),
      llmResp("Here is a hello-world function."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("Agent: Here is a hello-world function.");
  });

  it("includes Turn 1 heading", () => {
    const events: OmegaEvent[] = [userMsg("hello"), llmResp("hi"), turnEnd()];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("### Turn 1");
  });

  it("includes ## Session events heading", () => {
    const events: OmegaEvent[] = [userMsg("hello"), llmResp("hi"), turnEnd()];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("## Session events");
  });
});

describe("extractResumptionBasis — tool calls", () => {
  it("shows successful tool result as ok", () => {
    const events: OmegaEvent[] = [
      userMsg("Read a file"),
      llmResp("I'll read it."),
      toolCall("t1", "read_file", { path: "src/agent.ts" }),
      toolResult("t1", "read_file", "file content here"),
      llmResp("Done."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("read_file src/agent.ts → ok");
  });

  it("shows error tool result with first line of output", () => {
    const events: OmegaEvent[] = [
      userMsg("Run tests"),
      llmResp("Running tests."),
      toolCall("t1", "run_command", { command: "bun test" }),
      toolResult("t1", "run_command", "error: Cannot find module 'foo'\nmore detail", true),
      llmResp("There was an error."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("run_command bun test → error — error: Cannot find module 'foo'");
  });

  it("omits full tool output on success (only shows ok)", () => {
    const events: OmegaEvent[] = [
      userMsg("Do stuff"),
      llmResp("OK."),
      toolCall("t1", "read_file", { path: "README.md" }),
      toolResult("t1", "read_file", "A".repeat(5000)),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).not.toContain("A".repeat(100));
    expect(basis).toContain("→ ok");
  });
});

describe("extractResumptionBasis — errors", () => {
  it("includes agent_error", () => {
    const events: OmegaEvent[] = [
      userMsg("Do something"),
      agentError("Context too large"),
      turnInterrupted("error"),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("Error: Context too large");
  });

  it("includes turn_interrupted with error reason", () => {
    const events: OmegaEvent[] = [
      userMsg("Do something"),
      llmResp("Working..."),
      turnInterrupted("error"),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("[Turn interrupted due to error]");
  });

  it("does NOT include turn_interrupted with aborted reason", () => {
    const events: OmegaEvent[] = [
      userMsg("Do something"),
      llmResp("Working..."),
      turnInterrupted("aborted"),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).not.toContain("interrupted");
  });
});

describe("extractResumptionBasis — carry-forward from prior resumption", () => {
  it("includes carried-forward context section when session_resumed exists", () => {
    const priorSummary = "Auth module is complete. Next: write tests.";
    const events: OmegaEvent[] = [
      sessionResumed("2025-01-01T00-00-00-000-aaaaaaaa", priorSummary),
      userMsg("Write tests"),
      llmResp("Writing tests now."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("## Carried-forward context");
    expect(basis).toContain(priorSummary);
  });

  it("only processes events AFTER the last session_resumed", () => {
    const events: OmegaEvent[] = [
      // These events are before session_resumed and should be ignored
      userMsg("Old turn from before resumption"),
      llmResp("Old response"),
      turnEnd(),
      sessionResumed("2025-01-01T00-00-00-000-aaaaaaaa", "Prior summary."),
      userMsg("New turn after resumption"),
      llmResp("New response"),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).not.toContain("Old turn from before resumption");
    expect(basis).toContain("New turn after resumption");
  });

  it("uses the summary from session_resumed in the carry-forward section", () => {
    const events: OmegaEvent[] = [
      sessionResumed("2025-01-01T00-00-00-000-aaaaaaaa", "This is the summary."),
      userMsg("Continue"),
      llmResp("OK."),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("This is the summary.");
  });
});

describe("extractResumptionBasis — multiple turns", () => {
  it("numbers turns correctly", () => {
    const events: OmegaEvent[] = [
      userMsg("First"),
      llmResp("First reply"),
      turnEnd(),
      userMsg("Second"),
      llmResp("Second reply"),
      turnEnd(),
      userMsg("Third"),
      llmResp("Third reply"),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("### Turn 1");
    expect(basis).toContain("### Turn 2");
    expect(basis).toContain("### Turn 3");
  });

  it("includes content from all turns", () => {
    const events: OmegaEvent[] = [
      userMsg("First question"),
      llmResp("First answer"),
      turnEnd(),
      userMsg("Second question"),
      llmResp("Second answer"),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).toContain("First question");
    expect(basis).toContain("Second answer");
  });
});

describe("extractResumptionBasis — dropped events", () => {
  it("does not include server_started, session_started, llm_call, turn_end, llm_retry", () => {
    const events: OmegaEvent[] = [
      { type: "server_started", time: "2025-01-01T00:00:00.000Z" as any },
      { type: "session_started", time: "2025-01-01T00:00:00.000Z" as any, sessionId: "x", model: "m", authMode: "a", systemPrompt: "s" },
      userMsg("hello"),
      { type: "llm_call", time: "2025-01-01T00:00:01.000Z" as any, url: "u", model: "m", contextHashes: [], cacheBreakpointIndex: null, requestBytes: 100 },
      { type: "llm_retry", time: "2025-01-01T00:00:01.000Z" as any, attempt: 1, waitMs: 1000, error: "rate limit" },
      llmResp("response"),
      turnEnd(),
    ];
    const basis = extractResumptionBasis(events);
    expect(basis).not.toContain("server_started");
    expect(basis).not.toContain("session_started");
    expect(basis).not.toContain("llm_call");
    expect(basis).not.toContain("llm_retry");
    expect(basis).not.toContain("turn_end");
  });
});

// ---------------------------------------------------------------------------
// extractSummaryFromResponse
// ---------------------------------------------------------------------------

describe("extractSummaryFromResponse", () => {
  it("extracts text inside <summary> tags", () => {
    const raw = "Here is my analysis.\n<summary>\nThe work is done.\n</summary>\nEnd.";
    expect(extractSummaryFromResponse(raw)).toBe("The work is done.");
  });

  it("trims whitespace inside tags", () => {
    const raw = "<summary>  \n  content  \n  </summary>";
    expect(extractSummaryFromResponse(raw)).toBe("content");
  });

  it("falls back to full response when no tags present", () => {
    const raw = "  No tags here.  ";
    expect(extractSummaryFromResponse(raw)).toBe("No tags here.");
  });

  it("handles multiline content inside tags", () => {
    const raw = "<summary>Line 1\nLine 2\nLine 3</summary>";
    expect(extractSummaryFromResponse(raw)).toBe("Line 1\nLine 2\nLine 3");
  });
});

// ---------------------------------------------------------------------------
// generateSessionName
// ---------------------------------------------------------------------------

describe("generateSessionName", () => {
  it("calls provider with AUTO_NAME_INSTRUCTIONS as system and user/agent content", async () => {
    let capturedParams: any;
    const provider = makeNameStreamProvider("jwt login", (p) => { capturedParams = p; });
    await generateSessionName("Add JWT login", "I'll add JWT login to the app.", provider);
    expect(capturedParams.system).toBe(AUTO_NAME_INSTRUCTIONS);
    expect(capturedParams.messages[0].content).toContain("Add JWT login");
    expect(capturedParams.messages[0].content).toContain("I'll add JWT login to the app.");
  });

  it("returns sanitised lowercase name", async () => {
    const result = await generateSessionName("x", "y", makeNameStreamProvider("  JWT Login Endpoint!  "));
    expect(result).toBe("jwt login endpoint");
  });

  it("strips non-alphanumeric chars (except spaces)", async () => {
    const result = await generateSessionName("x", "y", makeNameStreamProvider("auth-tests: v2.0"));
    expect(result).toBe("authtests v20");
  });

  it("truncates very long names to 60 chars", async () => {
    const result = await generateSessionName("x", "y", makeNameStreamProvider("a".repeat(100)));
    expect(result.length).toBeLessThanOrEqual(60);
  });
});

// ---------------------------------------------------------------------------
// Agent.seedWithResumptionSummary — integration
// ---------------------------------------------------------------------------

function readEventsFile(path: string): OmegaEvent[] {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map(line => OmegaEventSchema.parse(JSON.parse(line)));
}

describe("Agent.seedWithResumptionSummary", () => {
  it("emits session_resumed event with correct fields", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent();
    afterAll(dispose);

    await agent.seedWithResumptionSummary(
      "The summary text.",
      "2025-01-01T00-00-00-000-aaaaaaaa",
    );
    await agent.flushEventLog();

    const events = readEventsFile(eventsFile);
    const ev = events.find(e => e.type === "session_resumed") as any;
    expect(ev).toBeDefined();
    expect(ev.continuationOf).toBe("2025-01-01T00-00-00-000-aaaaaaaa");
    expect(ev.summary).toBe("The summary text.");
    expect(typeof ev.time).toBe("string");
  });

  it("injects two synthetic messages into compactedContextHistory", async () => {
    const { agent, dispose } = await makeTestAgent();
    afterAll(dispose);

    await agent.seedWithResumptionSummary(
      "Summary of prior session.",
      "2025-01-01T00-00-00-000-aaaaaaaa",
    );

    const history = agent.getCompactedContextHistory() as any[];
    expect(history.length).toBe(2);
    expect(history[0]!.role).toBe("user");
    expect(history[1]!.role).toBe("assistant");
  });

  it("synthetic user message contains the summary", async () => {
    const { agent, dispose } = await makeTestAgent();
    afterAll(dispose);

    const summary = "Auth module complete. Tests pass. Next: deploy.";
    await agent.seedWithResumptionSummary(
      summary,
      "2025-01-01T00-00-00-000-aaaaaaaa",
    );

    const history = agent.getCompactedContextHistory() as any[];
    const userContent = typeof history[0]!.content === "string"
      ? history[0]!.content
      : JSON.stringify(history[0]!.content);
    expect(userContent).toContain(summary);
  });

  it("agent can continue sending messages after seeding", async () => {
    // After seeding, sendMessage should work normally (context is valid).
    let callCount = 0;
    const mockProvider = () => {
      callCount++;
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: "content_block_delta", delta: { type: "text_delta", text: "Hello!" } };
        },
        finalMessage: async () => ({
          id: "msg1",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          content: [{ type: "text", text: "Hello!" }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 12, output_tokens: 3 },
        }),
      };
    };

    const { agent, dispose } = await makeTestAgent(mockProvider as any);
    afterAll(dispose);

    await agent.seedWithResumptionSummary("Summary.", "old-session");

    const events: OmegaEvent[] = [];
    for await (const e of agent.sendMessage("Continue the work.", async () => true)) {
      events.push(e as OmegaEvent);
    }

    expect(callCount).toBe(1);
    expect(events.some(e => e.type === "turn_end")).toBe(true);
    expect(events.every(e => e.type !== "agent_error")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Agent.performResumption — integration
// ---------------------------------------------------------------------------

describe("Agent.performResumption", () => {
  it("logs resuming_session → llm_call → llm_response → session_resumed on success", async () => {
    const text = "<summary>Prior session summary.</summary><description>Did some work</description>";
    const { agent, eventsFile, dispose } = await makeTestAgent(makeTextStreamProvider(text));
    afterAll(dispose);
    await agent.init();

    for await (const _ of agent.performResumption("the basis text", "2025-01-01T00-00-00-000-aaaaaaaa")) {}
    await agent.flushEventLog();

    const events = readEventsFile(eventsFile);
    const types = events.map(e => e.type);

    // Verify the four events appear in the right order
    const startIdx = types.indexOf("resuming_session");
    const callIdx  = types.indexOf("llm_call");
    const respIdx  = types.indexOf("llm_response");
    const doneIdx  = types.indexOf("session_resumed");
    expect(startIdx).toBeGreaterThanOrEqual(0);
    expect(callIdx).toBeGreaterThan(startIdx);
    expect(respIdx).toBeGreaterThan(callIdx);
    expect(doneIdx).toBeGreaterThan(respIdx);
  });

  it("resuming_session event carries continuationOf and basis", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(makeTextStreamProvider("<summary>Summary.</summary>"));
    afterAll(dispose);
    await agent.init();

    for await (const _ of agent.performResumption("my basis", "old-session-dir")) {}
    await agent.flushEventLog();

    const events = readEventsFile(eventsFile);
    const ev = events.find(e => e.type === "resuming_session") as any;
    expect(ev.continuationOf).toBe("old-session-dir");
    expect(ev.basis).toBe("my basis");
  });

  it("llm_call event has correct model, contextHashes length and url", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(makeTextStreamProvider("<summary>S.</summary>"));
    afterAll(dispose);
    await agent.init();

    for await (const _ of agent.performResumption("basis", "old-dir")) {}
    await agent.flushEventLog();

    const events = readEventsFile(eventsFile);
    const ev = events.find(e => e.type === "llm_call") as any;
    expect(ev.model).toBe("claude-sonnet-4-6");
    expect(ev.url).toBe("https://api.anthropic.com/v1/messages");
    expect(ev.contextHashes).toHaveLength(1);
    expect(ev.cacheBreakpointIndex).toBeNull();
  });

  it("llm_response event carries usage and text", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(makeTextStreamProvider("<summary>Done.</summary>"));
    afterAll(dispose);
    await agent.init();

    for await (const _ of agent.performResumption("basis", "old-dir")) {}
    await agent.flushEventLog();

    const events = readEventsFile(eventsFile);
    const ev = events.find(e => e.type === "llm_response") as any;
    expect(ev.stopReason).toBe("end_turn");
    expect(ev.usage.input_tokens).toBe(10);
    expect(ev.text).toBe("<summary>Done.</summary>");
    expect(typeof ev.contextHash).toBe("string");
  });

  it("generator yields text chunks as StreamSignals during streaming", async () => {
    const { agent, dispose } = await makeTestAgent(makeTextStreamProvider("<summary>S.</summary>"));
    afterAll(dispose);
    await agent.init();

    const chunks: string[] = [];
    for await (const event of agent.performResumption("basis", "old")) {
      if (event.type === "text") chunks.push(event.text);
    }
    expect(chunks).toEqual(["<summary>S.</summary>"]);
  });

  it("llm_response text contains description tag for extraction", async () => {
    const text = "<summary>S.</summary><description>Added auth middleware</description>";
    const { agent, dispose } = await makeTestAgent(makeTextStreamProvider(text));
    afterAll(dispose);
    await agent.init();

    let description: string | undefined;
    for await (const event of agent.performResumption("basis", "old")) {
      if (event.type === "llm_response" && event.text) {
        description = extractDescriptionFromResponse(event.text);
      }
    }
    expect(description).toBe("Added auth middleware");
  });

  it("description is undefined when tag absent", async () => {
    const { agent, dispose } = await makeTestAgent(makeTextStreamProvider("<summary>S.</summary>"));
    afterAll(dispose);
    await agent.init();

    let description: string | undefined;
    for await (const event of agent.performResumption("basis", "old")) {
      if (event.type === "llm_response" && event.text) {
        description = extractDescriptionFromResponse(event.text);
      }
    }
    expect(description).toBeUndefined();
  });

  it("logs llm_error and re-throws when provider fails with non-retryable error", async () => {
    const err: any = new Error("Bad request");
    err.status = 400;
    const provider: StreamProvider = () => { throw err; };
    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    afterAll(dispose);
    await agent.init();

    await expect(
      (async () => { for await (const _ of agent.performResumption("basis", "old-dir")) {} })()
    ).rejects.toThrow("Bad request");

    await agent.flushEventLog();
    const events = readEventsFile(eventsFile);
    const errEv = events.find(e => e.type === "llm_error") as any;
    expect(errEv).toBeDefined();
    expect(errEv.error).toContain("Bad request");
    // session_resumed must NOT be present
    expect(events.some(e => e.type === "session_resumed")).toBe(false);
  });

  it("retries on transient error then succeeds", async () => {
    const saved = {
      base: process.env.OMEGA_RETRY_BASE_MS,
      max: process.env.OMEGA_RETRY_MAX_MS,
      attempts: process.env.OMEGA_RETRY_ATTEMPTS,
    };
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "3";

    let attempts = 0;
    const provider: StreamProvider = () => {
      attempts++;
      if (attempts < 3) {
        const err: any = new Error("overloaded");
        err.status = 529;
        throw err;
      }
      return makeTextStreamProvider("<summary>Recovered.</summary>")({} as any);
    };

    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    afterAll(dispose);
    await agent.init();

    for await (const _ of agent.performResumption("basis", "old-dir")) {}

    // Restore env
    const restore = (key: string, val: string | undefined) => {
      if (val === undefined) delete process.env[key];
      else process.env[key] = val;
    };
    restore("OMEGA_RETRY_BASE_MS", saved.base);
    restore("OMEGA_RETRY_MAX_MS", saved.max);
    restore("OMEGA_RETRY_ATTEMPTS", saved.attempts);

    await agent.flushEventLog();
    const events = readEventsFile(eventsFile);

    // Should have retried
    expect(attempts).toBe(3);
    const retryEvents = events.filter(e => e.type === "llm_retry");
    expect(retryEvents.length).toBe(2);
    // Should have succeeded
    expect(events.some(e => e.type === "session_resumed")).toBe(true);
    expect(events.some(e => e.type === "llm_response")).toBe(true);
  }, 30_000);

  it("respects abort signal during streaming", async () => {
    const controller = new AbortController();
    const provider: StreamProvider = () => ({
      async *[Symbol.asyncIterator]() {
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "chunk1 " } };
        // Abort fires here
        await new Promise(r => setTimeout(r, 20));
        controller.abort();
        await new Promise(r => setTimeout(r, 20));
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "chunk2" } };
      },
      finalMessage: async () => { throw new Error("should not reach finalMessage"); },
    });

    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);
    await agent.init();

    const yielded: any[] = [];
    for await (const event of agent.performResumption("basis", "old", controller.signal)) {
      yielded.push(event);
    }

    // Should not throw — abort is a clean return, not a throw
    // Should NOT have session_resumed (aborted before completion)
    expect(yielded.some(e => e.type === "session_resumed")).toBe(false);
  });
});
