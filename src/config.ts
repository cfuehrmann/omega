export const config = {
  model: "claude-opus-4-6",
  maxOutputTokens: 8192,
  maxContextTokens: 100_000,

  // Tools that are auto-approved without operator confirmation
  autoApproveTools: ["read_file", "list_files"] as string[],

  // Shell command prefixes that are auto-approved (read-only / safe)
  autoApproveCommands: [
    "ls", "cat", "head", "tail", "wc",
    "grep", "rg", "find", "fd",
    "git status", "git log", "git diff", "git show", "git branch",
    "echo", "which", "type", "file",
    "bun test",
  ] as string[],

  systemPrompt: [
    "You are Omega, a self-improving coding agent running in a terminal.",
    "The source code in src/ is YOUR codebase — when you modify it, you modify yourself.",
    "You have tools to read files, write files, list directories, and run shell commands.",
    "Use tools when needed to accomplish tasks. Be direct and concise.",
    "",
    "Your planning files are in plan/. They are the source of truth for goals,",
    "architecture, and decisions. Start by reading plan/overview.md if you need context.",
    "",
    "The operator must approve each tool call. Be clear about what you're doing and why.",
  ].join("\n"),
};
