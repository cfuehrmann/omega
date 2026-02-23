/**
 * Diagnostic snapshot writer.
 *
 * When a hard API error occurs (non-retryable, session-breaking), call
 * `writeDiagnosticWithBuffer()` to capture the full context at the moment of
 * failure: error body, request payload, conversation history, and a rolling
 * buffer of recent agent events that show what led up to the failure.
 *
 * The file is written to `diagnosis/` at the repo root and persists across
 * sessions so the next Omega instance can read it with hard data rather than
 * reconstructing from memory.
 *
 * At session start, `checkDiagnostics()` returns any existing diagnosis files
 * so the UI can warn the operator immediately.
 */

import { writeFile, mkdir, readdir } from "fs/promises";
import { join } from "path";

// ---------------------------------------------------------------------------
// BufferedEvent — the union of all event kinds tracked in the rolling buffer
// ---------------------------------------------------------------------------

export type BufferedEvent =
  | { type: "user_prompt"; content: string }
  | {
      type: "api_request";
      callNumber: number;
      provider: string;
      model: string;
      url?: string;
      /** Lightweight summary: number of messages, system length, etc. */
      requestSummary?: Record<string, unknown>;
    }
  | {
      type: "api_response";
      provider: string;
      stopReason: string;
      usage: { input_tokens: number; output_tokens: number };
    }
  | { type: "tool_call"; id: string; name: string; input: unknown }
  | {
      type: "tool_result";
      id: string;
      name: string;
      isError: boolean;
      /** First 500 chars of the result so the snapshot is readable. */
      outputPreview: string;
    }
  | { type: "agent_error"; message: string; retryable: boolean }
  | { type: "session_compacted"; turnCount: number; summaryLength: number };

// ---------------------------------------------------------------------------
// BufferedEntry — a BufferedEvent with metadata
// ---------------------------------------------------------------------------

export interface BufferedEntry {
  seqNo: number;
  ts: string; // ISO-8601
  event: BufferedEvent;
}

// ---------------------------------------------------------------------------
// RollingEventBuffer
// ---------------------------------------------------------------------------

/**
 * A fixed-capacity circular buffer of agent events.
 * When full, the oldest entry is evicted to make room.
 * Thread-safe within a single-threaded JS runtime.
 */
export class RollingEventBuffer {
  private readonly capacity: number;
  private readonly ring: (BufferedEntry | undefined)[];
  private head = 0; // index of the oldest item (when full)
  private size = 0;
  private seq = 0;

  constructor(capacity: number) {
    if (capacity < 1) throw new RangeError("capacity must be >= 1");
    this.capacity = capacity;
    this.ring = new Array(capacity);
  }

  /** Add an event to the buffer, evicting the oldest if at capacity. */
  push(event: BufferedEvent): void {
    const entry: BufferedEntry = {
      seqNo: ++this.seq,
      ts: new Date().toISOString(),
      event,
    };

    if (this.size < this.capacity) {
      // Buffer not yet full — append at head+size
      this.ring[(this.head + this.size) % this.capacity] = entry;
      this.size++;
    } else {
      // Buffer full — overwrite the oldest slot and advance head
      this.ring[this.head] = entry;
      this.head = (this.head + 1) % this.capacity;
    }
  }

  /** Return a snapshot of all entries in chronological order. */
  snapshot(): BufferedEntry[] {
    const result: BufferedEntry[] = [];
    for (let i = 0; i < this.size; i++) {
      result.push(this.ring[(this.head + i) % this.capacity]!);
    }
    return result;
  }

  /** Remove all entries. */
  clear(): void {
    this.head = 0;
    this.size = 0;
    this.ring.fill(undefined);
  }
}

// ---------------------------------------------------------------------------
// DiagnosticData
// ---------------------------------------------------------------------------

export interface DiagnosticData {
  /** One-line human-readable summary of what went wrong. */
  summary: string;
  /** The error message / body from the API (verbatim). */
  errorMessage: string;
  /** HTTP status code if available. */
  httpStatus?: number;
  /** Provider name ("anthropic" | "openai"). */
  provider: string;
  /** Model that was active when the error occurred. */
  model: string;
  /** API call number within the turn (1-based). */
  callNumber?: number;
  /** The exact `messages` array that was sent to the API. */
  requestMessages: unknown;
  /** System prompt blocks (without cache_control for readability). */
  systemBlocks?: unknown;
  /** Conversation history at the moment of failure (this.history). */
  history: unknown;
  /** Any additional structured context. */
  extra?: Record<string, unknown>;
}

const DEFAULT_DIAGNOSIS_DIR = "diagnosis";

// ---------------------------------------------------------------------------
// writeDiagnosticWithBuffer — the primary write path
// ---------------------------------------------------------------------------

/**
 * Write a diagnostic snapshot file that includes the rolling event buffer.
 *
 * @param data      Standard diagnostic fields (error, request, history…)
 * @param buffer    The rolling event buffer from the agent
 * @param diagDir   Override the output directory (used in tests)
 *
 * Returns the written path, or null if the write failed (errors are swallowed
 * so the caller never crashes because of the diagnostic writer).
 */
export async function writeDiagnosticWithBuffer(
  data: DiagnosticData,
  buffer: RollingEventBuffer,
  diagDir: string = DEFAULT_DIAGNOSIS_DIR,
): Promise<string | null> {
  try {
    await mkdir(diagDir, { recursive: true });

    const ts = new Date().toISOString().replace(/[:.]/g, "-").replace("Z", "Z");
    const filename = `${ts}.json`;
    const path = join(diagDir, filename);

    const snapshot = {
      _omega_diagnostic: true,
      timestamp: new Date().toISOString(),
      summary: data.summary,
      provider: data.provider,
      model: data.model,
      httpStatus: data.httpStatus ?? null,
      callNumber: data.callNumber ?? null,
      errorMessage: data.errorMessage,

      // Rolling buffer: everything that led up to this error — user prompts,
      // API calls/responses, tool executions, errors, compaction events.
      eventBuffer: buffer.snapshot(),

      // The exact messages array sent to the API — the most important artifact
      requestMessages: data.requestMessages,
      systemBlocks: data.systemBlocks ?? null,
      // Full in-memory history at moment of failure
      history: data.history,
      extra: data.extra ?? null,
      _instructions: [
        "Read this file at the start of a debugging session.",
        "eventBuffer shows the full sequence of events leading up to the failure.",
        "requestMessages is what was literally sent to the API.",
        "history is the agent's in-memory conversation history.",
        "Compare them: are there orphaned tool_result blocks?",
        "Are tool_use IDs in assistant messages matched by tool_result IDs?",
        "Delete this file once the bug is diagnosed and fixed.",
      ],
    };

    await writeFile(path, JSON.stringify(snapshot, null, 2), "utf-8");
    return path;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// writeDiagnostic — backwards-compatible shim (no buffer)
// ---------------------------------------------------------------------------

/**
 * Legacy write path — writes a diagnostic snapshot without an event buffer.
 * Kept for call-sites that haven't been migrated yet.
 *
 * @deprecated Prefer `writeDiagnosticWithBuffer`.
 */
export async function writeDiagnostic(data: DiagnosticData): Promise<string | null> {
  const emptyBuffer = new RollingEventBuffer(1);
  return writeDiagnosticWithBuffer(data, emptyBuffer);
}

// ---------------------------------------------------------------------------
// checkDiagnostics
// ---------------------------------------------------------------------------

/**
 * Return paths of any existing diagnosis files, sorted oldest-first.
 * Returns an empty array if the directory doesn't exist or is empty.
 */
export async function checkDiagnostics(): Promise<string[]> {
  try {
    const entries = await readdir(DEFAULT_DIAGNOSIS_DIR);
    return entries
      .filter(e => e.endsWith(".json"))
      .sort()
      .map(e => join(DEFAULT_DIAGNOSIS_DIR, e));
  } catch {
    return [];
  }
}
