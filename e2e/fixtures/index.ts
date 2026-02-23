/**
 * Custom Playwright fixtures for Omega e2e tests.
 *
 * Provides a `server` fixture with helpers that talk to the control HTTP API
 * exposed by e2e/fixtures/test-server.ts (running on port 3002).
 * The actual test server (port 3001) is started by playwright.config.ts's
 * webServer option as a Bun subprocess.
 */

import { test as base, expect } from "@playwright/test";

const CTRL = "http://localhost:3002";

export interface ServerHelper {
  /** Send a JSON event to the browser WebSocket client */
  sendEvent(event: object): Promise<void>;
  /** Drain and return all messages received from the browser */
  drainMessages(): Promise<string[]>;
  /** Wait for the next message from the browser and parse it as JSON */
  nextMessage(): Promise<unknown>;
  /** Reset event log, received messages, AND disk persistence file */
  reset(): Promise<void>;
  /** Flush in-memory event log to disk (simulates clean shutdown) */
  save(): Promise<void>;
  /** Reload event log from disk, replacing in-memory state (simulates restart) */
  load(): Promise<void>;
  /** Return the current disk snapshot without modifying in-memory state */
  diskSnapshot(): Promise<unknown>;
}

async function sendEvent(event: object): Promise<void> {
  await fetch(`${CTRL}/control/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ event }),
  });
}

async function drainMessages(): Promise<string[]> {
  const res = await fetch(`${CTRL}/control/messages`);
  return res.json() as Promise<string[]>;
}

async function nextMessage(): Promise<unknown> {
  // Poll until a message arrives (max 5s)
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const msgs = await drainMessages();
    if (msgs.length > 0) return JSON.parse(msgs[0]);
    await new Promise(r => setTimeout(r, 50));
  }
  throw new Error("Timed out waiting for message from browser");
}

async function reset(): Promise<void> {
  await fetch(`${CTRL}/control/reset`, { method: "POST" });
}

async function save(): Promise<void> {
  await fetch(`${CTRL}/control/save`, { method: "POST" });
}

async function load(): Promise<void> {
  await fetch(`${CTRL}/control/load`, { method: "POST" });
}

async function diskSnapshot(): Promise<unknown> {
  const res = await fetch(`${CTRL}/control/disk-snapshot`);
  return res.json();
}

export interface Fixtures {
  server: ServerHelper;
}

export const test = base.extend<Fixtures>({
  server: async ({}, use) => {
    await reset();
    const helper: ServerHelper = { sendEvent, drainMessages, nextMessage, reset, save, load, diskSnapshot };
    await use(helper);
  },
});

export { expect };
