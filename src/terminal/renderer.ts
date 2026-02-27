/**
 * Terminal block renderers — ANSI styling and structured output.
 *
 * No Agent imports, no key-handling logic.
 * Exported for use by the terminal app and for unit tests.
 */

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

const CSI = "\x1b[";

function sgr(...codes: number[]) {
  return `${CSI}${codes.join(";")}m`;
}

const RESET  = sgr(0);
const BOLD   = sgr(1);
const DIM_ON = sgr(2);

function styled(s: string, ...codes: number[]): string {
  return sgr(...codes) + s + RESET;
}

export function bold   (s: string) { return styled(s, 1); }
export function dim    (s: string) { return styled(s, 2); }
export function green  (s: string) { return styled(s, 32); }
function cyan   (s: string) { return styled(s, 36); }
function blue   (s: string) { return styled(s, 34); }
export function yellow (s: string) { return styled(s, 33); }
function magenta(s: string) { return styled(s, 35); }
export function red    (s: string) { return styled(s, 31); }

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

export const TIME_WIDTH = 10;
export const INDENT  = "  ";
export const INDENT2 = "    ";
const INDENT3 = "      ";

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

export function now(): string {
  return new Date().toLocaleTimeString("en-GB");
}

function truncateOutput(text: string, maxLines = 10): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… [${lines.length - maxLines} more lines]`;
}

/** Print lines to stdout, first line gets timestamp, rest get indent. */
export function printBlock(time: string, lines: string[]): void {
  for (let i = 0; i < lines.length; i++) {
    const prefix = i === 0
      ? dim(time.padEnd(TIME_WIDTH))
      : " ".repeat(TIME_WIDTH);
    process.stdout.write(prefix + lines[i] + "\n");
  }
}

export function println(s: string): void {
  process.stdout.write(s + "\n");
}

// ---------------------------------------------------------------------------
// Block renderers
// ---------------------------------------------------------------------------

export function renderUserMessage(content: string): string[] {
  return [
    bold(green("user_message")),
    ...content.split("\n").map(line => green(`${INDENT}${line}`)),
  ];
}

function summariseContent(content: any): string {
  if (typeof content === "string") {
    return content.length <= 60 ? `"${content}"` : `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  const parts = (content as any[]).map((b: any) => {
    if (b.type === "text")        return `text: <${b.text.length} chars>`;
    if (b.type === "tool_use")    return `tool_use: "${b.name}"`;
    if (b.type === "tool_result") return `tool_result: <${(b.content as string).length} chars>`;
    return b.type;
  });
  return `[${parts.join(", ")}]`;
}

export function renderApiRequest(
  provider: "anthropic" | "openai",
  url: string,
  request: any,
): string[] {
  if (provider === "anthropic") {
    const messages: any[] = request.messages ?? [];
    const n = messages.length;
    const last = messages[n - 1];
    const prev = n >= 2 ? messages[n - 2] : null;
    const prevSummary = prev ? `{ role: "${prev.role}", content: ${summariseContent(prev.content)} }` : null;
    const lastSummary = last ? `{ role: "${last.role}", content: ${summariseContent(last.content)} }` : null;
    const msgLine = n === 0
      ? "messages: []"
      : prevSummary
        ? `messages[${n}]: ${prevSummary} … ${lastSummary}`
        : `messages[${n}]: … ${lastSummary}`;
    return [bold(cyan("llm_call")), dim(cyan(`${INDENT}${msgLine}`))];
  } else {
    const input: any[] = request.input ?? [];
    const n = input.length;
    const last = input[n - 1];
    const prev = n >= 2 ? input[n - 2] : null;
    const prevSummary = prev ? summariseOpenAiInput(prev) : null;
    const lastSummary = last ? summariseOpenAiInput(last) : null;
    const msgLine = n === 0
      ? "input: []"
      : prevSummary
        ? `input[${n}]: ${prevSummary} … ${lastSummary}`
        : `input[${n}]: … ${lastSummary}`;
    return [bold(cyan("llm_call")), dim(cyan(`${INDENT}${msgLine}`))];
  }
}

function summariseOpenAiInput(item: any): string {
  if (item.role && typeof item.content === "string") {
    return `{ role: "${item.role}", content: ${item.content.length <= 60 ? `"${item.content}"` : `<${item.content.length} chars>`} }`;
  }
  if (item.type === "function_call") {
    return `{ type: "function_call", name: "${item.name}", arguments: <${(item.arguments ?? "").length} chars> }`;
  }
  if (item.type === "function_call_output") {
    return `{ type: "function_call_output", call_id: "${item.call_id}", output: <${(item.output ?? "").length} chars> }`;
  }
  return `{ ${JSON.stringify(item).slice(0, 80)} }`;
}

export function renderApiResponse(
  provider: "anthropic" | "openai",
  url: string,
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: any[],
  raw?: any,
): string[] {
  const lines: string[] = [];
  lines.push(bold(blue("llm_response")));
  if (provider === "anthropic") {
    lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
    lines.push(blue(`${INDENT}usage:`));
    lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
    lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
    lines.push(blue(`${INDENT}content:`));
  } else {
    lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
    lines.push(blue(`${INDENT}usage:`));
    lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
    lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
    if (raw?.usage?.cached_input_tokens !== undefined) {
      lines.push(dim(blue(`${INDENT2}cached_input_tokens: ${raw.usage.cached_input_tokens}`)));
    }
    if (raw?.usage?.cache_creation_input_tokens !== undefined) {
      lines.push(dim(blue(`${INDENT2}cache_creation_input_tokens: ${raw.usage.cache_creation_input_tokens}`)));
    }
    lines.push(blue(`${INDENT}content:`));
  }
  for (const block of content) {
    if (block.type === "text") {
      lines.push(blue(`${INDENT2}text:`));
      const preview = block.text.length <= 120 ? block.text : `<${block.text.length} chars>`;
      for (const line of preview.split("\n")) {
        lines.push(dim(blue(`${INDENT3}${line}`)));
      }
    } else if (block.type === "tool_use") {
      lines.push(blue(`${INDENT2}tool_use:`));
      lines.push(dim(blue(`${INDENT3}name: "${block.name}"`)));
      lines.push(dim(blue(`${INDENT3}input: ${JSON.stringify(block.input)}`)));
    } else {
      lines.push(dim(blue(`${INDENT2}${block.type}`)));
    }
  }
  return lines;
}

/** Returns a short display suffix for a tool call ID (last 6 chars). */
function shortId(id: string): string {
  return id.length <= 6 ? id : `…${id.slice(-6)}`;
}

/** Render the start of a tool call (name + input only). Shown immediately when the call is issued. */
export function renderToolStart(name: string, input: any, id: string): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`agent_to_agent_tool_call`)) + dim(yellow(`  [${shortId(id)}]`)));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  lines.push(yellow(`${INDENT}input: ${JSON.stringify(input)}`));
  return lines;
}

/** Render the result of a tool call. Shown with a fresh timestamp when the tool completes. */
export function renderToolResult(result: { output: string; isError: boolean }, id: string): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`agent_to_agent_tool_result`)) + dim(yellow(`  [${shortId(id)}]`)));
  lines.push(dim(yellow(`${INDENT}is_error: ${result.isError}`)));
  lines.push(dim(yellow(`${INDENT}content:`)));
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push(dim(yellow(`${INDENT2}${line}`)));
  }
  return lines;
}

export function renderToolResultMessage(
  results: Array<{ tool_use_id: string; content: string; is_error: boolean }>,
): string[] {
  const lines: string[] = [];
  lines.push(bold(magenta("tool_result_message")));
  lines.push(magenta(`${INDENT}role: "user"`));
  lines.push(magenta(`${INDENT}content:`));
  for (const r of results) {
    lines.push(magenta(`${INDENT2}tool_result:`));
    lines.push(dim(magenta(`${INDENT3}tool_use_id: "${r.tool_use_id}"`)));
    lines.push(dim(magenta(`${INDENT3}is_error: ${r.is_error}`)));
    lines.push(dim(magenta(`${INDENT3}content: <${r.content.length} chars>`)));
  }
  return lines;
}

export function renderAssistantMessage(text: string, dimText?: string): string[] {
  const lines: string[] = text.split("\n");
  if (dimText) {
    lines.push(dim(dimText));
  }
  return lines;
}

export function renderStatus(streaming: boolean): string {
  return dim(streaming ? "Esc to interrupt" : "Ctrl+C to quit  /sonnet /opus /codex /compact /help");
}
