export const config = {
  model: "claude-opus-4-20250514",
  maxOutputTokens: 8192,
  maxContextTokens: 100_000,
  systemPrompt: [
    "You are Omega, a coding assistant.",
    "Your project's planning files are in `plan/`. They are the source of",
    "truth for goals, architecture, and decisions. If you lose context,",
    "`ls plan/` and re-read the files.",
  ].join(" "),
};
