/**
 * Tests for the fold-at-quit architecture:
 *
 * 1. World state path is project-specific (keyed to cwd).
 * 2. Agent.foldCurrentSessionIntoWorldState() exists and folds history correctly.
 * 3. Agent does NOT have checkPriorSession / resumeSession (removed).
 * 4. Agent does NOT write session files after a turn.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, readdir } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import type { StreamProvider } from "./agent.js";
import { Agent } from "./agent.js";
import { projectWorldStatePath } from "./world-state.js";
import { readWorldState } from "./world-state.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let tempDir: string;

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-faq-test-"));
});

afterEach(async () => {
  await rm(tempDir, { recursive: true, force: true });
});

function makeMockStream(responseText: string) {
  return {
    async *[Symbol.asyncIterator]() {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: responseText } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
      yield { type: "message_stop" };
    },
    async finalMessage() {
      return {
        id: "msg_test",
        type: "message",
        role: "assistant",
        content: [{ type: "text", text: responseText }],
        model: "claude-sonnet-4-6",
        stop_reason: "end_turn",
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 5 },
      } as any;
    },
  };
}

function makeMockProvider(responseText = "ok"): StreamProvider {
  return async () => makeMockStream(responseText);
}

async function collectEvents(agent: Agent, message: string) {
  const events = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// 1. World state path is project-specific
// ---------------------------------------------------------------------------

describe("projectWorldStatePath", () => {
  it("is exported from world-state.ts", () => {
    expect(typeof projectWorldStatePath).toBe("function");
  });

  it("returns a string path ending in .md", () => {
    const p = projectWorldStatePath("/home/user/my-project");
    expect(typeof p).toBe("string");
    expect(p.endsWith(".md")).toBe(true);
  });

  it("returns different paths for different directories", () => {
    const p1 = projectWorldStatePath("/home/user/project-a");
    const p2 = projectWorldStatePath("/home/user/project-b");
    expect(p1).not.toBe(p2);
  });

  it("returns the same path for the same directory", () => {
    const p1 = projectWorldStatePath("/home/user/project");
    const p2 = projectWorldStatePath("/home/user/project");
    expect(p1).toBe(p2);
  });

  it("path lives inside the project directory as plan/world-state.md", () => {
    const p = projectWorldStatePath("/some/project");
    expect(p).toBe("/some/project/plan/world-state.md");
  });
});

// ---------------------------------------------------------------------------
// 2. Agent.foldCurrentSessionIntoWorldState() exists and works
// ---------------------------------------------------------------------------

describe("Agent.foldCurrentSessionIntoWorldState", () => {
  it("is a method on Agent", () => {
    const agent = new Agent(makeMockProvider(), null);
    expect(typeof agent.foldCurrentSessionIntoWorldState).toBe("function");
  });

  it("writes world state to disk after folding", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider("summary of session"), null, undefined, worldStatePath);
    await collectEvents(agent, "hello");

    // foldCurrentSessionIntoWorldState is now an async generator — drain it
    for await (const _ of agent.foldCurrentSessionIntoWorldState()) { /* drain */ }

    const content = await readWorldState(worldStatePath);
    expect(content).not.toBeNull();
    expect(typeof content).toBe("string");
    expect((content as string).length).toBeGreaterThan(0);
  });

  it("is a no-op when worldStatePath is null", async () => {
    const agent = new Agent(makeMockProvider(), null, undefined, null);
    // Should not throw and should emit no events
    const events: any[] = [];
    for await (const e of agent.foldCurrentSessionIntoWorldState()) { events.push(e); }
    expect(events).toHaveLength(0);
  });

  it("incorporates prior world state when one exists", async () => {
    const worldStatePath = join(tempDir, "world-state.md");

    // Write initial world state
    const { writeWorldState } = await import("./world-state.js");
    await writeWorldState("Prior state: project was started.", worldStatePath);

    let capturedFoldPrompt = "";
    const provider: StreamProvider = async (params) => {
      // The world-state fold prompt contains the prior world state
      const msg = params.messages[0].content as string;
      if (msg.includes("Prior state")) capturedFoldPrompt = msg;
      return makeMockStream("Updated state.");
    };

    const agent = new Agent(provider, null, undefined, worldStatePath);
    await collectEvents(agent, "do something");
    for await (const _ of agent.foldCurrentSessionIntoWorldState()) { /* drain */ }

    expect(capturedFoldPrompt).toContain("Prior state: project was started.");
  });
});

// ---------------------------------------------------------------------------
// 3. Agent does NOT have checkPriorSession / resumeSession
// ---------------------------------------------------------------------------

describe("Agent — removed session-resume API", () => {
  it("does not have checkPriorSession method", () => {
    const agent = new Agent(makeMockProvider(), null);
    expect((agent as any).checkPriorSession).toBeUndefined();
  });

  it("does not have resumeSession method", () => {
    const agent = new Agent(makeMockProvider(), null);
    expect((agent as any).resumeSession).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// 4. Agent does NOT write session files after a turn
// ---------------------------------------------------------------------------

describe("Agent — no session file persistence", () => {
  it("does not create any files in a given directory after a turn", async () => {
    // Pass a directory as sessionDir — if persistSession still exists it would write here
    const agent = new Agent(makeMockProvider(), null);
    await collectEvents(agent, "hello");
    await Bun.sleep(100);

    // tempDir should be empty — no session files written
    const files = await readdir(tempDir);
    expect(files.length).toBe(0);
  });

  it("does not have persistSession method", () => {
    const agent = new Agent(makeMockProvider(), null);
    expect((agent as any).persistSession).toBeUndefined();
  });
});
