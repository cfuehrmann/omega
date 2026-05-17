# Prompt architecture findings — Omega vs. four reference agents

**Status:** complete  
**Origin investigation:** `2026-05-17` session  
**Companion:** `backlog/prompt-architecture-comparison.md` (investigation spec)

---

## 1. Summary table

| Project | Core chars | Core words | Core ~tokens | Core sections | Self-dev AGENTS.md chars | AGENTS.md ~tokens | AGENTS.md sections | Combined ~tokens |
|---|---|---|---|---|---|---|---|---|
| **Omega** | 7,454 | 1,153 | **1,863** | 7 | 1,861 | **465** | 4 | **2,328** |
| Forge | 7,299 | 1,038 | 1,824 | 6 | 7,331 | 1,832 | 15 | 3,656 |
| Pi (static)¹ | 1,172 | 153 | 293 | 0 | 11,434 | 2,858 | 37 | 3,151 |
| OpenCode (default) | 8,661 | 1,418 | 2,165 | 7 | 2,646 | 661 | 8 | 2,826 |
| OpenCode (anthropic) | 8,212 | 1,335 | 2,053 | 6 | (same) | (same) | (same) | 2,714 |
| Claude Code | n/a (closed binary) | — | — | — | not in repo | — | — | — |

> ¹ Pi's core is a template literal; the 1,172-char figure covers the static skeleton only. At runtime
> `toolsList` + `guidelines` + optional `contextFiles`, `skills`, and `appendSystemPrompt` are injected,
> easily doubling or tripling the figure. Pi is deliberately thin statically and fat dynamically.
>
> Token estimates use the `chars / 4` heuristic throughout.

---

### Section bucket distribution

Rows are projects; columns are content buckets. ✓ = present, (✓) = partial/thin.

| Project | Identity | Tool ergonomics | Output format | Workflow discipline | Bug-fix policy | Project-specific | Meta / self |
|---|---|---|---|---|---|---|---|
| Omega core | (✓) | ✓✓ | ✓✓ | ✓ | ✓ | — | ✓ |
| Omega AGENTS.md | — | — | — | ✓ | — | ✓ | ✓ |
| Forge core | (✓) | ✓ | (✓) | ✓✓ | — | — | — |
| Forge AGENTS.md | — | — | — | ✓ | ✓✓ | ✓✓ | — |
| Pi core | (✓) | (injected) | — | (injected) | — | (✓) | — |
| Pi AGENTS.md | — | — | — | ✓✓ | ✓ | ✓✓ | — |
| OpenCode default | ✓ | ✓✓ | ✓✓ | ✓ | — | — | (✓) |
| OpenCode AGENTS.md | — | — | — | ✓ | — | ✓✓ | (✓) |
| Claude Code | n/a | — | — | — | — | — | — |

---

## 2. Per-project notes

### Omega
Omega's core sits in the middle of the size range (1,863 tokens), close to Forge and well below
OpenCode. The core is dominated by the `## Tools` section, which accounts for roughly 40 % of the
text and provides unusually detailed per-tool guidance (bias, truncation_bias, wait_for_output
semantics, edit_file discipline). This depth is a deliberate trade-off: the tool set is bespoke
(not Claude Code's tools, not pi's tools) so the guidance cannot be assumed. The `## Output format`
section is the second-largest bucket, covering Mermaid/C4 specifics that are genuinely
output-format decisions rather than project knowledge. Both fit the core properly. The
**`## LLM Provider` section** (≈ 400 chars, ≈ 100 tokens) names Omega's supported models and
explains how to look up Anthropic docs; it is the one section that is squarely *product-specific*
rather than general coding-agent behaviour. Omega's self-dev **AGENTS.md is by far the lightest**
of the four measurable projects — 465 tokens vs. the next-lightest (OpenCode) at 661 tokens and
Pi's 2,858 tokens. This is not obviously wrong, but the disparity is large enough to warrant
checking whether project-specific knowledge has been left out.

### Forge
Forge's core (forge.md body, 1,824 tokens) is nearly identical in size to Omega's. The balance is
different: Forge invests heavily in `# Task Management` (todo-tool discipline, examples with XML
traces) and largely defers tool-specific guidance to prose over the tool-selection section.
Forge's **AGENTS.md is 4× heavier** than Omega's (1,832 tokens) and carries substantial
project-specific content: Rust error-handling conventions (`anyhow`, `thiserror`), a detailed
test-writing recipe using `pretty_assertions` and `derive_setters`, service-architecture
anti-patterns with code examples, and documentation rules. All of this is rightly in AGENTS.md
rather than in the core — it only applies to the Forge codebase. The split is clean: the core
carries general agent behaviour; the AGENTS.md carries codebase conventions.

### Pi
Pi's static core is exceptionally small (≈ 293 tokens) because it is designed as a harness
rather than a finished agent: it injects the tool list, guidelines, and optionally custom prompts
at runtime. The `customPrompt` option can replace the default entirely, and the `appendSystemPrompt`
and `contextFiles` options extend it. Pi's **AGENTS.md is the largest** of any project measured
(2,858 tokens, 37 sections) and covers a very wide surface: TypeScript code-quality rules,
detailed `npm run check` discipline, multi-agent Git safety rules (no `git add -A`, forbidden
operations), PR workflow, contributor-gate operation, lockstep versioning, a full guide for adding
a new LLM provider, and how to test the TUI with tmux. The breadth reflects that pi is a large
multi-package monorepo maintained by a small team with strict contribution policies — the AGENTS.md
is the team's living style guide.

### OpenCode
OpenCode's core (default.txt, 2,165 tokens) is the heaviest among the measurable cores. Its
distinguishing feature is the `# Tone and style` section, which is unusually detailed: multiple
examples of verbosity-level calibration (`2 + 2 → 4`, `write tests for new feature → [uses
tools]`), a strong anti-preamble rule, and a 4-line limit enforced with imperative language.
The `anthropic.txt` variant swaps this for a shorter tone section and adds `# Professional
objectivity` (avoid superlatives, apply rigorous standards, investigate before confirming beliefs)
— a section absent from every other project. OpenCode also has a `# Code References` section
requiring `file_path:line_number` patterns, which none of the other agents codify in the core.
The **AGENTS.md is lean** (661 tokens) relative to the codebase's complexity: it covers style
guides (destructuring, ternaries, Drizzle schema naming), testing rules, and branch conventions.
The leanness is by design — OpenCode uses AGENTS.md for conventions that change, not for
tool-behaviour policies.

### Claude Code (reference only)
The core prompt is baked into a closed binary and is not measurable. From the public docs:
Claude Code reads CLAUDE.md files (not AGENTS.md by default; AGENTS.md can be imported via
`@AGENTS.md`). Its memory system has four tiers — org-wide, user-level, project, and
project-local — loaded in order from broadest to narrowest. The docs recommend keeping each
CLAUDE.md under 200 lines; longer files "consume more context and reduce adherence." The public
repo (`anthropics/claude-code`) has no CLAUDE.md or AGENTS.md at the root, only a
`.claude/commands/` directory for slash commands.

---

## 3. Omega-specific verdict

### Core prompt: right-sized
At 1,863 tokens Omega is squarely in the middle of the peer group (Pi static excluded as
incomparable). The only structural outlier is `## LLM Provider`, which names three Omega-specific
model handles and a docs-lookup recipe. Everything else — tools, output format, design discipline,
bug-fix policy, task-completion policy — is generic coding-agent behaviour that belongs in the core
by the reasoning established in the origin session.

### AGENTS.md: under-sized
Omega's self-dev AGENTS.md is 4× lighter than the next-lightest peer and 6× lighter than Pi.
The current content (commit rule, workflow, contract-authority data-model hierarchy, tricky-bugs
heuristic) is correct but incomplete. The most notable gap is the absence of **project-specific
coding conventions**: Forge's AGENTS.md carries detailed Rust patterns for tests and services
targeted at the Forge codebase; Pi's carries TypeScript style rules and multi-agent git safety
targeted at the pi monorepo. Omega has none of this. Whether this is a problem depends on whether
the Omega codebase has conventions worth encoding. Given that Omega is itself a Rust project with
consistent patterns for error handling, test style, and service architecture, there is likely
convention worth capturing.

The `## LLM Provider` section of the core is the one content that is Omega-self-development
knowledge rather than general agent behaviour.

---

## 4. Proposed edits

### Proposal 1 — Move `## LLM Provider` from core prompt to `AGENTS.md`

**Rationale.** The section names the three models Omega ships (`claude-sonnet-4-6`,
`claude-opus-4-6`, `claude-opus-4-7`) and explains how to resolve Anthropic docs. When Omega works
on a Java project, a Python script, or anything other than its own codebase, knowing Omega's
supported model list has zero task value. The content is product-knowledge, not agent-behaviour.
Moving it to AGENTS.md makes it active only in Omega self-development sessions, saves ≈ 100 tokens
from every non-Omega session, and aligns with the peer pattern (Forge's Rust service patterns live
in its AGENTS.md; they are not in the core that ships to all users).

**The docs-lookup recipe** ("fetch `https://platform.claude.com/llms.txt`…") is borderline: it is
a tool-use hint about Anthropic's own documentation tree, useful in any session that involves
Claude API questions. Option A: move it with the rest of the section (simpler, only Omega sessions
need it). Option B: keep just the docs-lookup recipe in core (1-2 lines) and move the model list.
Recommendation: Option A — move the whole section. The URL is stable enough to appear in
AGENTS.md, and the overall savings and pattern consistency outweigh the marginal risk of losing
the hint in non-Omega sessions.

**File:** `rust/crates/omega-agent/src/system_prompt.rs` — remove the `## LLM Provider` block
from `core_prompt()`.

**File:** `AGENTS.md` — append a `## LLM Provider` section with the same text.

**Before (core):** ~400 chars, the full `## LLM Provider` block  
**After (core):** removed (saves ~100 tokens per non-Omega session)  
**After (AGENTS.md):** grows from 1,861 → ~2,261 chars (still the lightest in the peer group)

---

### Proposal 2 — Grow `AGENTS.md` with Omega Rust conventions (separate, lower-priority)

**Rationale.** The comparison with Forge reveals that Omega's AGENTS.md carries no project-specific
Rust conventions, despite the codebase having clear patterns (e.g., test style visible in
`system_prompt.rs` — `tempfile`, env-injected helpers, unit tests in the same file). If the
team finds that Omega repeatedly gets Rust test style, error-handling (e.g., `anyhow` vs. custom
errors), or module organisation wrong during self-development sessions, the right fix is to add
those conventions to AGENTS.md rather than to the core. This is a lower-priority proposal because
it requires a separate authoring effort rather than a simple relocation.

**Scope:** a `## Rust conventions` (or equivalent) section in `AGENTS.md`, covering at minimum:
test placement (same file), preferred test helper patterns, `anyhow`/`thiserror` usage, and
module-naming conventions. Looking at the peers, 10-20 bullet points would put Omega at roughly
Forge-AGENTS.md density, which is appropriate for a codebase of similar size.

---

### No change to block order, stacking semantics, or core-vs-baked decision

These are settled (see `backlog/prompt-architecture-comparison.md § Decisions already made`).
The comparison confirms them: all four measurable peers bake the core, all stack AGENTS.md, and
all use an ordering equivalent to Omega's (core → runtime → AGENTS.md tiers).
