# Prompt architecture comparison — Omega vs. four reference agents

**Status:** investigation pending — open a fresh session to do this.
**Origin session:** `2026-05-16T21-06-13-670-cfb0fb27` (see for full context).
**Owner:** Carsten.

## Why this exists

In the origin session we restructured Omega's core prompt and `AGENTS.md`
and reached a decision (Position A: keep coding-agent framing in the
core prompt) but **did not** measure how Omega's prompt mass compares
to the reference agents'. Before tweaking further, we want concrete
numbers and a structural map.

The follow-up to this investigation may be: shifting content between
Omega's core prompt and Omega's repo `AGENTS.md` (the one used when
working on Omega itself), adjusting overall size, or no change at all.

## Decisions already made — do NOT relitigate

1. **Stacking vs. override for `AGENTS.md`**: stacking, no override.
   All four references (Forge, Pi, OpenCode, Claude Code) do this; no
   reason to be different.
2. **Position A (coding-agent framing in core prompt)**: adopted.
   Evidence: TB2 is 100% software-engineering tasks; OpenClaw ships
   `@earendil-works/pi-coding-agent` 0.74.1 unchanged as the engine of
   an explicitly general-purpose product, so the framing is not a
   bottleneck.
3. **Core prompt stays baked into source** (not loaded from
   `AGENTS.md`). All four references do this.
4. **Block order in Omega**: core → runtime context → global
   `AGENTS.md` → repo `AGENTS.md`. Stays.

## Goals of the new session

Produce a written analysis (markdown, in this same `backlog/` dir or as
a reply summary) covering:

1. **Sizes** of each agent's core prompt (chars, words, approximate
   tokens, section count).
2. **Sizes** of each agent's *self-development* `AGENTS.md` — the file
   used when an agent works on its own codebase. (For projects that
   don't have one, note that and skip.)
3. **Section taxonomy**: for each prompt, classify content into rough
   buckets — e.g. *identity*, *tool ergonomics*, *output format*,
   *workflow discipline*, *bug-fix policy*, *project-specific*,
   *meta-instructions about the prompt itself*. Same buckets across
   all five so the comparison is apples-to-apples.
4. **Omega in the landscape**: where does Omega sit on each axis?
   Larger / smaller / similar? Which buckets are over- or
   under-represented?
5. **Concrete proposals**: zero or more specific shifts of content
   between Omega's core prompt and Omega's repo `AGENTS.md`, with
   justification grounded in the comparison. "No change" is a valid
   conclusion if the numbers say so.

## Where the prompts live

| Project | Core prompt | Self-dev `AGENTS.md` |
|---|---|---|
| Omega | `/home/carsten/omega/dev/rust/crates/omega-agent/src/system_prompt.rs` (Rust string constant; entry point `build_system_blocks()` / `join_blocks()`) | `/home/carsten/omega/dev/AGENTS.md` |
| Forge | `/home/carsten/forgecode/crates/forge_repo/src/agents/{forge,muse,sage}.md` (each a full markdown file with YAML frontmatter; `forge.md` is the default coding agent) | `/home/carsten/forgecode/AGENTS.md` if present (check) |
| Pi | `/home/carsten/pi-mono/packages/coding-agent/src/core/system-prompt.ts` (function `buildSystemPrompt()`; the literal default starts at line 131: *"You are an expert coding assistant…"*) | `/home/carsten/pi-mono/AGENTS.md` if present (check) |
| OpenCode | `/home/carsten/opencode/packages/opencode/src/session/prompt/{default,anthropic,gpt,kimi,gemini,codex,…}.txt` (per-model). Dispatched in `src/session/instruction.ts`. | `/home/carsten/opencode/AGENTS.md` if present (check) |
| Claude Code | Not on disk (baked into Anthropic's binary). Skip the core comparison; cite the structure documented at <https://docs.claude.com/en/docs/claude-code/memory> for context. | `claude-code` is closed-source; their public self-dev AGENTS.md is on github.com/anthropics/claude-code if it exists. |

Note: OpenCode has multiple per-model core prompts — compare them to
each other briefly, then pick `default.txt` (or `anthropic.txt`,
whichever is the primary) as the canonical entry for the cross-project
comparison.

Also: Pi has a `customPrompt` option that *replaces* the default
entirely. OpenClaw could be using this but on inspection
(origin session) does not — it ships pi's coding-assistant prompt
verbatim. Useful data point; don't re-investigate.

## Methodology hints

- Counts: `wc -c -w <file>` for chars + words; approximate tokens as
  `chars / 4` for the table (don't bother with a real tokenizer unless
  the new session wants to).
- The Rust core prompt is a multi-line string constant. Either extract
  it to a temp file or eyeball the string boundaries; either is fine.
- For OpenCode `.txt` files: they ARE the whole prompt, easy.
- For Pi: the prompt is partly templated (skills, tools injected). For
  the size comparison, measure the *static* part (the literal string
  starting line 131 plus the static framework around it). Note in the
  writeup that templating injects additional content at runtime.
- For Forge: each `agents/*.md` has a YAML frontmatter and a body. The
  body is the system prompt template. Measure the body only.

## Output format

Write the analysis as `backlog/prompt-architecture-findings.md` in this
repo. Suggested structure:

1. Summary table (rows = projects, columns = core chars, core sections,
   self-dev AGENTS.md chars, bucket distribution).
2. Per-project paragraph (3-6 sentences) on what's distinctive.
3. Omega-specific verdict: is the core prompt too long, too short, or
   right-sized? Same for `AGENTS.md`. Any concrete moves to propose?
4. List of proposed edits (file path + before/after sketch) or
   "no changes recommended" with reasoning.

Then commit. Doc-only path → gate is skipped.

## Out of scope

- Don't redesign the stacking semantics.
- Don't relitigate Position A vs. B.
- Don't measure runtime token usage in real sessions — static prompt
  size only.
- Don't propose new tools, new AGENTS.md layers, or new block
  injection sites.

## After the investigation

If the analysis recommends edits, do them in a *separate* follow-up
session (one focused task per session). The investigation session's
job is the analysis document, not the edits.
