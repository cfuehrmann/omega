import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, readdir } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import { Agent, type OmegaEvent, type StreamSignal } from "./agent.js";
import type { StreamProvider } from "./agent.js";

function authExpiredError() {
  const err: any = new Error('401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}}');
  err.status = 401;
  return err;
}

function rateLimitError(message = "rate limit: try again in 0.01s") {
  const err: any = new Error(message);
  err.status = 429;
  return err;
}

async function collectEvents(agent: Agent, message: string, signal?: AbortSignal): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(message, async () => true, signal)) {
    events.push(event);
  }
  return events;
}

describe("rate limit backoff", () => {
  it("OpenAI retries on rate limit and succeeds", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "3";

    let openAiCalls = 0;
    const openAiCaller = async () => {
      openAiCalls += 1;
      if (openAiCalls < 3) throw rateLimitError("Please try again in 0.01s");
      return {
        response: {
          content: [{ type: "text", text: "ok" } as any],
          stop_reason: "stop",
          usage: { input_tokens: 1, output_tokens: 2 },
        },
        text: "ok",
        raw: { usage: { input_tokens: 1, output_tokens: 2 } },
      };
    };

    const agent = new Agent(undefined, null, openAiCaller as any, null, null, null);
    agent.setProvider("openai");
    const events = await collectEvents(agent, "hello");

    expect(openAiCalls).toBe(3);
    const errors = events.filter((e) => e.type === "agent_error") as any[];
    if (errors.length > 0) {
      expect(errors[errors.length - 1].error).not.toContain("/sonnet");
    }

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("OpenAI gives up after retries and suggests /sonnet or /opus", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const openAiCaller = async () => {
      throw rateLimitError("Please try again in 0.01s");
    };

    const agent = new Agent(undefined, null, openAiCaller as any, null, null, null);
    agent.setProvider("openai");
    const events = await collectEvents(agent, "hello");

    const errors = events.filter((e) => e.type === "agent_error") as any[];
    const error = errors[errors.length - 1];
    expect(error).toBeTruthy();
    expect(error.error).toContain("/sonnet");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("Anthropic gives up after retries and suggests /codex", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const mockProvider: StreamProvider = async () => {
      throw rateLimitError();
    };

    const agent = new Agent(mockProvider, null);
    agent.setProvider("anthropic");
    const events = await collectEvents(agent, "hello");

    const errors = events.filter((e) => e.type === "agent_error") as any[];
    const error = errors[errors.length - 1];
    expect(error).toBeTruthy();
    expect(error.error).toContain("/codex");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});

describe("OAuth token expiry reauth", () => {
  it("retries after 401 when reinitAuth succeeds", async () => {
    // First call throws 401; second call (after mock reauth) succeeds.
    // Note: the provider may also be called for post-turn compaction — we track
    // only whether the API call sequence had exactly one 401 before success.
    let llmCallCount = 0;
    let hadAuthError = false;
    let hadSuccess = false;

    const mockProvider: StreamProvider = async () => {
      llmCallCount += 1;
      if (llmCallCount === 1) {
        hadAuthError = true;
        throw authExpiredError();
      }
      hadSuccess = true;
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "ok" } };
        },
        async finalMessage() {
          return {
            id: "msg_ok",
            type: "message",
            role: "assistant",
            content: [{ type: "text", text: "ok" }],
            model: "claude-sonnet-4-6",
            stop_reason: "end_turn",
            stop_sequence: null,
            usage: { input_tokens: 5, output_tokens: 2 },
          } as any;
        },
      };
    };

    const agent = new Agent(mockProvider, null);
    // Override reinitAuth so it doesn't touch the real token file
    (agent as any).reinitAuth = async () => {
      (agent as any).authMode = "oauth"; // stays oauth
      return true; // pretend refresh succeeded
    };

    const events = await collectEvents(agent, "hello");
    expect(hadAuthError).toBe(true);
    expect(hadSuccess).toBe(true);
    const texts = events.filter(e => e.type === "text").map((e: any) => e.text);
    expect(texts.join("")).toContain("ok");
    // No agent_error event — oauth_token_expired and oauth_refreshed typed events are ok
    const errors = events.filter(e => e.type === "agent_error") as any[];
    expect(errors).toHaveLength(0);
  });

  it("reports login required when reinitAuth fails", async () => {
    const mockProvider: StreamProvider = async () => {
      throw authExpiredError();
    };

    const agent = new Agent(mockProvider, null);
    // Override reinitAuth to fail
    (agent as any).reinitAuth = async () => false;

    const events = await collectEvents(agent, "hello");
    const errors = events.filter(e => e.type === "agent_error") as any[];
    expect(errors.length).toBeGreaterThan(0);
    const lastError = errors[errors.length - 1];
    expect(lastError.error).toContain("login");
  });
});

// ---------------------------------------------------------------------------
// Context overflow: errors out immediately, no retry, writes diagnostic
// ---------------------------------------------------------------------------

describe("context overflow (prompt too long)", () => {
  let tempDir: string;

  beforeEach(async () => {
    tempDir = await mkdtemp(join(tmpdir(), "omega-ptl-diag-test-"));
  });

  afterEach(async () => {
    await rm(tempDir, { recursive: true, force: true });
  });

  function promptTooLongError() {
    const err: any = new Error('400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 250000 tokens"}}');
    err.status = 400;
    return err;
  }

  it("emits llm_error + actionable agent_error and writes diagnostic — no retry", async () => {
    const diagDir = join(tempDir, "diagnosis");
    let callCount = 0;

    const mockProvider: StreamProvider = async () => {
      callCount++;
      throw promptTooLongError();
    };

    const agent = new Agent(mockProvider, null, undefined, diagDir);
    const events = await collectEvents(agent, "hello");

    // Must NOT retry — only one API call
    expect(callCount).toBe(1);

    // Must yield llm_error
    const llmErrors = events.filter(e => e.type === "llm_error");
    expect(llmErrors.length).toBe(1);

    // Must yield actionable agent_error mentioning /compact
    const errorEvents = events.filter(e => e.type === "agent_error") as any[];
    expect(errorEvents.length).toBeGreaterThanOrEqual(1);
    expect(errorEvents.some(e => e.error.includes("compact"))).toBe(true);

    // Diagnostic file must have been written
    const diagFiles = await readdir(diagDir);
    const jsonFiles = diagFiles.filter(f => f.endsWith(".json"));
    expect(jsonFiles.length).toBe(1);

    const diagContent = await Bun.file(join(diagDir, jsonFiles[0]!)).json();
    expect(diagContent.summary).toContain("Context overflow");
    expect(diagContent.errorMessage).toContain("prompt is too long");
    expect(diagContent.httpStatus).toBe(400);
    expect((diagContent.extra as any).stopReason).toBe("context_overflow");
    expect(diagContent.logFile).toBeUndefined();
    expect(diagContent.eventBuffer).toBeUndefined();
  });

  it("also errors out cleanly for isContextTooLong (429 extra usage required)", async () => {
    function contextTooLongError() {
      const err: any = new Error("429 Extra usage is required for long context requests");
      err.status = 429;
      return err;
    }

    const diagDir = join(tempDir, "diagnosis2");
    let callCount = 0;
    const mockProvider: StreamProvider = async () => {
      callCount++;
      throw contextTooLongError();
    };

    const agent = new Agent(mockProvider, null, undefined, diagDir);
    const events = await collectEvents(agent, "hello");

    // No retry — but note: 429 is also retryable, so the retry loop fires first.
    // isContextTooLong check comes after the retryable check exhausts attempts.
    // We just verify the error path produces a diagnostic and agent_error.
    const errorEvents = events.filter(e => e.type === "agent_error") as any[];
    expect(errorEvents.length).toBeGreaterThanOrEqual(1);

    let diagFiles: string[] = [];
    try { diagFiles = await readdir(diagDir); } catch { /* ok */ }
    const jsonFiles = diagFiles.filter(f => f.endsWith(".json"));
    expect(jsonFiles.length).toBeGreaterThanOrEqual(1);
  });
});
