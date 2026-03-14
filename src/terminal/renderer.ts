/**
 * Terminal block renderers — ANSI styling and structured output.
 *
 * No Agent imports, no key-handling logic.
 * Exported for use by the terminal app and for unit tests.
 *
 * Colors follow Catppuccin Mocha (24-bit / truecolor ANSI).
 */

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

const CSI = "\x1b[";

function sgr(...codes: number[]) {
  return `${CSI}${codes.join(";")}m`;
}

const RESET = sgr(0);

function styled(s: string, ...codes: number[]): string {
  return sgr(...codes) + s + RESET;
}

/** 24-bit foreground color. */
function fg(r: number, g: number, b: number): string {
  return `${CSI}38;2;${r};${g};${b}m`;
}

/** Wrap text in a 24-bit foreground color + optional bold/dim, then reset. */
function color(s: string, r: number, g: number, b: number, mods: number[] = []): string {
  const modCodes = mods.length ? sgr(...mods) : "";
  return modCodes + fg(r, g, b) + s + RESET;
}

// ---------------------------------------------------------------------------
// Catppuccin Mocha palette
// ---------------------------------------------------------------------------

// Accent colors
const MOCHA_BLUE     = [137, 180, 250] as const; // #89b4fa  — user messages
const MOCHA_SAPPHIRE = [116, 199, 236] as const; // #74c7ec  — llm_call + llm_response
const MOCHA_TEAL     = [148, 226, 213] as const; // #94e2d5  — (available)
const MOCHA_GREEN    = [166, 227, 161] as const; // #a6e3a1  — prompt, ok states
const MOCHA_YELLOW   = [249, 226, 175] as const; // #f9e2af  — tool_call + tool_result
const MOCHA_PEACH    = [250, 179, 135] as const; // #fab387  — (available)
const MOCHA_RED      = [243, 139, 168] as const; // #f38ba8  — errors
const MOCHA_MAUVE    = [203, 166, 247] as const; // #cba6f7  — status / misc

// Neutral / surface
const MOCHA_TEXT     = [205, 214, 244] as const; // #cdd6f4  — primary text
const MOCHA_SUBTEXT1 = [186, 194, 222] as const; // #bac2de
const MOCHA_OVERLAY2 = [147, 153, 178] as const; // #9399b2  — dim text
const MOCHA_OVERLAY1 = [127, 132, 156] as const; // #7f849c  — dimmer

// ---------------------------------------------------------------------------
// Named color helpers (matching the logical roles used in the app)
// ---------------------------------------------------------------------------

export function bold   (s: string) { return styled(s, 1); }
export function dim    (s: string) { return color(s, ...MOCHA_OVERLAY2); }

// Semantic role → Mocha color
export function green  (s: string) { return color(s, ...MOCHA_GREEN); }
export function yellow (s: string) { return color(s, ...MOCHA_YELLOW); }
export function red    (s: string) { return color(s, ...MOCHA_RED); }

// llm_call / llm_response share the same sapphire accent
function sapphire(s: string) { return color(s, ...MOCHA_SAPPHIRE); }
function sapphireBold(s: string) { return color(s, ...MOCHA_SAPPHIRE, [1]); }
function sapphireDim(s: string)  { return color(s, ...MOCHA_OVERLAY1); }

// tool_call / tool_result share the yellow accent
function yellowBold(s: string) { return color(s, ...MOCHA_YELLOW, [1]); }
function yellowDim(s: string)  { return color(s, ...MOCHA_OVERLAY1); }

// user_message
function blue  (s: string) { return color(s, ...MOCHA_BLUE); }

// status / misc
function mauve (s: string) { return color(s, ...MOCHA_MAUVE); }

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

export const TIME_WIDTH = 10;
export const INDENT  = "  ";
export const INDENT2 = "    ";

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

export function now(): string {
  return new Date().toLocaleTimeString("en-GB");
}

function truncateOutput(text: string, maxLines = 5, maxChars = 500): string {
  const lines = text.split("\n");
  const linesCut = lines.length > maxLines;
  const charsCut = text.length > maxChars;
  if (!linesCut && !charsCut) return text;

  // Apply whichever limit fires first.
  let result = text;
  let note = "";
  if (linesCut && (!charsCut || lines.slice(0, maxLines).join("\n").length <= maxChars)) {
    result = lines.slice(0, maxLines).join("\n");
    note = `… [${lines.length} lines / ${text.length} chars total — showing first ${maxLines} lines]`;
  } else {
    result = text.slice(0, maxChars);
    note = `… [${lines.length} lines / ${text.length} chars total — showing first ${maxChars} chars]`;
  }
  return result + "\n" + note;
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

export function renderUserMessage(): string[] {
  return [bold(blue("user_message"))];
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
  model?: string,
): string[] {
  const modelSuffix = model ? dim(`  [${model}]`) : "";
  if (provider === "anthropic") {
    const messages: any[] = request?.messages ?? [];
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
    return [
      sapphireBold("llm_call") + modelSuffix,
      sapphireDim(`${INDENT}${msgLine}`),
    ];
  } else {
    const input: any[] = request?.input ?? [];
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
    return [
      sapphireBold("llm_call") + modelSuffix,
      sapphireDim(`${INDENT}${msgLine}`),
    ];
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
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens?: number | null;
    cache_read_input_tokens?: number | null;
    service_tier?: string | null;
  },
  model?: string,
): string[] {
  const modelSuffix = model ? dim(`  [${model}]`) : "";
  const lines: string[] = [];
  lines.push(sapphireBold("llm_response") + modelSuffix);
  lines.push(sapphire(`${INDENT}stop_reason: "${stopReason}"`));
  lines.push(sapphire(`${INDENT}usage:`));
  lines.push(sapphireDim(`${INDENT2}input_tokens: ${usage.input_tokens}`));
  lines.push(sapphireDim(`${INDENT2}output_tokens: ${usage.output_tokens}`));
  if (usage.cache_creation_input_tokens) lines.push(sapphireDim(`${INDENT2}cache_write: ${usage.cache_creation_input_tokens}`));
  if (usage.cache_read_input_tokens)     lines.push(sapphireDim(`${INDENT2}cache_read: ${usage.cache_read_input_tokens}`));
  if (usage.service_tier && usage.service_tier !== "standard") lines.push(sapphireDim(`${INDENT2}service_tier: ${usage.service_tier}`));
  return lines;
}

/** Returns a short display suffix for a tool call ID (last 6 chars). */
function shortId(id: string): string {
  return id.length <= 6 ? id : `…${id.slice(-6)}`;
}

/** Render the start of a tool call (name + input only). Shown immediately when the call is issued. */
export function renderToolStart(name: string, input: any, id: string): string[] {
  const lines: string[] = [];
  lines.push(yellowBold(`tool_call`) + yellowDim(`  [${shortId(id)}]`));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  for (const line of truncateOutput(JSON.stringify(input)).split("\n")) {
    lines.push(yellow(`${INDENT}${line}`));
  }
  return lines;
}

/** Render the result of a tool call. Shown with a fresh timestamp when the tool completes. */
export function renderToolResult(result: { output: string; isError: boolean }, id: string): string[] {
  const lines: string[] = [];
  lines.push(yellowBold(`tool_result`) + yellowDim(`  [${shortId(id)}]`));
  lines.push(yellowDim(`${INDENT}is_error: ${result.isError}`));
  lines.push(yellowDim(`${INDENT}content:`));
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push(yellowDim(`${INDENT2}${line}`));
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
