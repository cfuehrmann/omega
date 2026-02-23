/**
 * Tests for WEB-6: world-state fold on web server shutdown.
 *
 * Verifies that the web server's shutdown path calls
 * foldCurrentSessionIntoWorldState() on the active agent, mirroring what
 * terminal/app.ts does on Ctrl+C / SIGTERM.
 *
 * Uses a mock Agent to avoid real LLM calls.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, writeFile, mkdir } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import type { AgentEvent } from "../agent.js";
import { performWebShutdown } from "./server.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let tempDir: string;

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-web-shutdown-test-"));
});

afterEach(async () => {
  await rm(tempDir, { recursive: true, force: true });
});

function makeMockAgent(foldEvents: AgentEvent[]): any {
  return {
    foldCurrentSessionIntoWorldState: async function* () {
      for (const event of foldEvents) {
        yield event;
      }
    },
  };
}

// ---------------------------------------------------------------------------
// performWebShutdown
// ---------------------------------------------------------------------------

describe("performWebShutdown", () => {
  it("calls foldCurrentSessionIntoWorldState on the agent", async () => {
    let foldCalled = false;
    const agent = {
      foldCurrentSessionIntoWorldState: async function* () {
        foldCalled = true;
      },
    } as any;

    await performWebShutdown(agent);
    expect(foldCalled).toBe(true);
  });

  it("drains all events from the fold generator", async () => {
    const foldEvents: AgentEvent[] = [
      { type: "api_call_start", callNumber: 1, provider: "anthropic", url: "https://api.anthropic.com", request: {} as any },
      { type: "world_state_saved", path: "/tmp/world.md", charCount: 42 },
    ];
    const collected: AgentEvent[] = [];
    const agent = {
      foldCurrentSessionIntoWorldState: async function* () {
        for (const e of foldEvents) {
          collected.push(e);
          yield e;
        }
      },
    } as any;

    await performWebShutdown(agent);
    expect(collected).toHaveLength(2);
    expect(collected[0].type).toBe("api_call_start");
    expect(collected[1].type).toBe("world_state_saved");
  });

  it("does not throw when the fold emits no events (empty history)", async () => {
    const agent = makeMockAgent([]);
    await expect(performWebShutdown(agent)).resolves.toBeUndefined();
  });

  it("does not throw when the fold emits an error event", async () => {
    const agent = makeMockAgent([
      { type: "error", error: "LLM exploded" } as AgentEvent,
    ]);
    await expect(performWebShutdown(agent)).resolves.toBeUndefined();
  });

  it("tolerates a null/undefined agent (no active session)", async () => {
    await expect(performWebShutdown(null as any)).resolves.toBeUndefined();
    await expect(performWebShutdown(undefined as any)).resolves.toBeUndefined();
  });
});
