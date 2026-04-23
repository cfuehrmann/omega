/**
 * Headless CLI entrypoint for Omega.
 *
 * Designed for benchmarking (Harbor / Terminal-Bench 2) and scripted use.
 * Runs the agent loop to completion with no web server, no TUI, and no
 * interactive prompts. All events are written to events.jsonl inside the
 * session directory.
 *
 * Usage:
 *   bun run src/cli.ts run \
 *     --instruction "Fix the failing tests" \
 *     --model claude-sonnet-4-6 \
 *     --session-dir /tmp/omega-session \
 *     [--effort medium] \
 *     [--max-turns 50]
 *
 * Exit codes:
 *   0  — agent completed normally (turn_end)
 *   1  — turn interrupted (abort / LLM error), or invalid arguments
 */

import { mkdir, writeFile } from "fs/promises";
import { join } from "path";
import { Agent, makeDefaultCreateMessageStream } from "./agent.js";
import { makeSessionDir, SESSIONS_ROOT } from "./session-dir.js";
import { config } from "./config.js";
import type { OmegaEvent, StreamSignal } from "./events.js";

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

function parseArgs(argv: string[]): {
  subcommand: string | undefined;
  instruction: string | undefined;
  model: string;
  effort: string;
  sessionDir: string | undefined;
  maxTurns: number | undefined;
  help: boolean;
} {
  const args = argv.slice(2);

  function getArg(flag: string): string | undefined {
    const idx = args.indexOf(flag);
    return idx !== -1 ? args[idx + 1] : undefined;
  }

  const maxTurnsRaw = getArg("--max-turns");
  const maxTurns =
    maxTurnsRaw !== undefined ? parseInt(maxTurnsRaw, 10) : undefined;

  return {
    subcommand: args[0],
    instruction: getArg("--instruction"),
    model: getArg("--model") ?? config.model,
    effort: getArg("--effort") ?? "medium",
    sessionDir: getArg("--session-dir"),
    maxTurns:
      maxTurns !== undefined && !isNaN(maxTurns) ? maxTurns : undefined,
    help: args.includes("--help") || args.includes("-h"),
  };
}

// ---------------------------------------------------------------------------
// Session directory setup
// ---------------------------------------------------------------------------

/**
 * Prepare the session directory and return paths to context and event files.
 * If an explicit path is given, create it and place the files there.
 * Otherwise fall back to the normal `.omega/sessions/<timestamp>/` layout.
 */
async function prepareSessionPaths(
  sessionDir: string | undefined,
): Promise<{ dir: string; contextFile: string; eventsFile: string }> {
  if (sessionDir !== undefined) {
    await mkdir(sessionDir, { recursive: true });
    const contextFile = join(sessionDir, "context.jsonl");
    const eventsFile = join(sessionDir, "events.jsonl");
    // Touch the files so they exist from the start (mirrors makeSessionDir).
    await writeFile(contextFile, "", { flag: "a" });
    await writeFile(eventsFile, "", { flag: "a" });
    return { dir: sessionDir, contextFile, eventsFile };
  }
  return makeSessionDir(new Date(), SESSIONS_ROOT);
}

// ---------------------------------------------------------------------------
// Progress reporting (stderr, non-intrusive)
// ---------------------------------------------------------------------------

function log(msg: string): void {
  process.stderr.write(msg + "\n");
}

function formatEvent(event: OmegaEvent | StreamSignal): string | null {
  switch (event.type) {
    case "session_started":
      return `[session] ${event.sessionId}  model=${event.model}  effort=${event.effort}`;
    case "tool_call":
      return `[tool] ${event.name}`;
    case "tool_result":
      return `[tool_result] ${event.name}  ${event.isError ? "ERROR" : "ok"}  ${event.durationMs}ms`;
    case "llm_call":
      return `[llm_call] model=${event.model}`;
    case "llm_retry":
      return `[llm_retry] attempt=${event.attempt}  wait=${event.waitMs}ms`;
    case "llm_error":
      return `[llm_error] ${event.error}`;
    case "turn_end":
      return `[turn_end] input=${event.metrics.inputTokens}  output=${event.metrics.outputTokens}`;
    case "turn_interrupted":
      return `[turn_interrupted] reason=${event.reason ?? "unknown"}`;
    case "agent_error":
      return `[agent_error] ${event.error}`;
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const HELP = `\
Usage: bun run src/cli.ts run --instruction <text> [options]

Options:
  --instruction <text>   Task to run (required, or pipe to stdin)
  --model <id>           Model ID (default: ${config.model})
  --effort <level>       Thinking effort: low|medium|high|max|xhigh (default: medium)
  --session-dir <path>   Directory for events.jsonl / context.jsonl
                         (default: .omega/sessions/<timestamp>/)
  --max-turns <n>        Abort after N LLM API calls (runaway protection)
  --help, -h             Show this help

Example:
  bun run src/cli.ts run \\
    --instruction "Fix the failing tests" \\
    --model claude-sonnet-4-6 \\
    --session-dir /tmp/omega-session \\
    --max-turns 50
`;

async function main(): Promise<number> {
  const opts = parseArgs(process.argv);

  if (opts.help || opts.subcommand === undefined) {
    process.stderr.write(HELP);
    return opts.help ? 0 : 1;
  }

  if (opts.subcommand !== "run") {
    process.stderr.write(`Unknown subcommand: ${opts.subcommand}. Expected "run".\n\n${HELP}`);
    return 1;
  }

  // Read instruction from --instruction or stdin
  let instruction = opts.instruction;
  if (!instruction) {
    if (process.stdin.isTTY) {
      process.stderr.write(
        "Error: --instruction is required (or pipe to stdin)\n\n" + HELP,
      );
      return 1;
    }
    const chunks: string[] = [];
    for await (const chunk of process.stdin) {
      chunks.push(new TextDecoder().decode(chunk as Uint8Array));
    }
    instruction = chunks.join("").trim();
    if (!instruction) {
      process.stderr.write("Error: instruction is empty\n");
      return 1;
    }
  }

  const { dir, contextFile, eventsFile } = await prepareSessionPaths(
    opts.sessionDir,
  );

  const createMessageStream = makeDefaultCreateMessageStream();
  const agent = new Agent(createMessageStream, contextFile, eventsFile, dir);
  agent.setModel(opts.model);
  agent.setEffort(opts.effort);

  await agent.init();
  await agent.loadSystemPromptAppend();

  const abortCtrl = new AbortController();

  // Graceful shutdown
  const handleShutdown = () => {
    log("[shutdown] Signal received — aborting turn");
    abortCtrl.abort();
  };
  process.on("SIGINT", handleShutdown);
  process.on("SIGTERM", handleShutdown);

  let llmCallCount = 0;
  let exitCode = 0;
  // Track whether we're mid-stream so we can emit a newline before the next
  // structured log line (which starts with '[').
  let midStream = false;

  const logLine = (msg: string): void => {
    if (midStream) {
      process.stderr.write("\n");
      midStream = false;
    }
    log(msg);
  };

  try {
    for await (const event of agent.sendMessage(
      instruction,
      async () => true, // auto-approve all tools
      abortCtrl.signal,
    )) {
      // Stream text/thinking directly to stderr without a newline so output
      // appears inline as the model generates it.
      if (event.type === "text") {
        process.stderr.write(event.text);
        midStream = true;
        continue;
      }
      if (event.type === "thinking") {
        // Thinking is verbose — skip it in headless mode.
        continue;
      }

      const line = formatEvent(event as OmegaEvent | StreamSignal);
      if (line) logLine(line);

      if ("type" in event) {
        const e = event as OmegaEvent;

        // Budget enforcement: allow up to --max-turns complete LLM responses,
        // then abort before the next call starts.  Counting llm_response (not
        // llm_call) means each counted turn is fully complete — the model's
        // output and any tool results are in the session before we stop.
        if (e.type === "llm_response") {
          llmCallCount++;
          if (opts.maxTurns !== undefined && llmCallCount >= opts.maxTurns) {
            logLine(
              `[budget] Reached --max-turns=${opts.maxTurns} — aborting`,
            );
            abortCtrl.abort();
          }
        }

        // Determine exit code from terminal events
        if (e.type === "turn_interrupted") exitCode = 1;
        if (e.type === "agent_error") exitCode = 1;
        if (e.type === "llm_error") exitCode = 1;
      }
    }
  } finally {
    await agent.emitServerStopped(exitCode === 0 ? "clean" : "error");
    await agent.flushEventLog();
    log(`[done] events.jsonl → ${eventsFile}`);
  }

  return exitCode;
}

main().then((code) => process.exit(code)).catch((err: unknown) => {
  process.stderr.write(
    `Fatal: ${err instanceof Error ? err.message : String(err)}\n`,
  );
  process.exit(1);
});
