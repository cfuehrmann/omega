# Orientation Prefix Analysis

**Date:** 2026-05-16 (revised)
**Corpus:** 174 Omega sessions (`.omega/sessions/`) + 309 Harbor bench sessions (`bench/jobs/`) = **483 total**
**Script:** `bench/scripts/analyze_orientation.py`

---

## Context

The question: can we find common "orientation prefixes" in sessions — early tool calls the agent makes before doing real work — that could be pre-loaded into context to save turns?

The broader motivation is a data-driven approach to project memory files: instead of hardcoding what to load, let the session data tell us what actually gets read at session start, and whether a `prompt type → orientation prefix` mapping is stable enough to be useful.

---

## What "the claude memory slot" is

Not a separate API parameter. The Anthropic Messages API allows `system` to be an **array of typed content blocks** rather than a plain string:

```json
{
  "system": [
    { "type": "text", "text": "Core system prompt...", "cache_control": { "type": "ephemeral" } },
    { "type": "text", "text": "AGENTS.md content...",  "cache_control": { "type": "ephemeral" } }
  ]
}
```

Memory files in Claude Code are just additional entries in that array, each independently cacheable and structurally labeled by source. No special API; just the array form of `system`.

---

## How comparable agents handle it

| Agent | Loads AGENTS.md? | How stored | Separate system blocks? | Cache per block? |
|---|---|---|---|---|
| **opencode** | ✅ auto (tree walk + URLs) | Array, one element per file, prefixed `Instructions from: <path>` | ✅ Yes — each file → own AI SDK system block → own Anthropic content block | ✅ Yes |
| **forgecode** | ✅ auto (3 fixed paths) | Template-rendered into main system prompt string | ❌ One block | ❌ |
| **pi-mono** | ✅ auto (tree walk, `--no-context-files` to opt out) | String-concatenated under `# Project Context` header | ❌ One block | ❌ |
| **Omega** | ❌ agent discovers via tool calls | n/a | n/a | n/a |

Omega is the only one that makes the agent fetch orientation files with tool calls. All three comparators pre-load at session start.

---

## Methodology note — corpus boundary (revised 2026-05-16)

An earlier version of this analysis ran over **all** of `.omega/sessions/` (765 entries) and reported `run_command(sleep 10)` as the most common first tool call (33 %). That finding was an artifact of TypeScript-era mock-test sessions (`abort_sleep_test` fixture) that were intermixed with real sessions in the same directory. A separate investigation established that:

- The Rust CLI replaced the TypeScript agent on **2026-05-02 23:38** (commit `d2ac588`, first usable Rust binary on 2026-05-01 22:04; t=0 chosen as the first Rust-format session strictly after the last TypeScript-format session).
- The real Rust/TS discriminator is the **sessionId format** in the `session_started` event:
  - TypeScript: `1775334042480-flefnt` (Unix-ms + 6-char random suffix)
  - Rust: `2026-05-13T20-33-03-029-d158c41e` (ISO timestamp matching directory name)
- Since t=0 there has been **zero mock-test pollution** in `.omega/sessions/` — Harbor sessions go to `bench/jobs/`, the Rust mock server writes to `.omega/test-sessions/`, and the production binary is the only writer of `.omega/sessions/`.

All 678 pre-t=0 sessions (and one orphan 2026-03-21 flat `events.jsonl`) have been moved to `.omega/sessions-archive-ts/`. The analyzer also applies a defensive Rust-clean filter on load (`_is_rust_clean_session`) so future TS-era artifacts can't silently re-enter the analysis. Harbor sessions were never contaminated and required no filtering.

---

## Key findings from session analysis

### 1. Harbor: no orientation prefix makes sense

Harbor tasks already deliver the full spec in the prompt. The agent's first moves are **environment probing**, not instruction-reading:

- `list_files(/app)`, `run_command(ls -la /app/)`, `run_command(cd /app && …)` to discover what files exist
- Then task-specific jumps (if the prompt names a file, the agent reads it directly)

Only **1.0 % of Harbor sessions** read `instruction.md`/`README` as the very first call; **1.3 %** within the first 3 calls. The container environment is unknown at dispatch time, so there is nothing useful to pre-load.

### 2. Omega: clear canonical orientation pattern

The single most common first tool call across **all 483 sessions** (Omega + Harbor combined) is `list_files(/.)` at **97 (20.1 %)** — these are all Omega sessions, since `/app` is what Harbor agents see (shown as `/` in reports due to path normalisation), `/.` is what Omega agents see in the repo. Followed by Harbor's `list_files(/app)` (normalised to `/`) at **56 (11.6 %)**.

When Omega sessions orient, the dominant continuation is:

```
list_files(/.) → find_files(AGENT.md|README*) → [read README.md | list subdirs]
```

Specific 3-call prefix counts (across the full corpus):

| Count | Prefix |
|---:|---|
| 8 | `list_files(/.) → find_files(README*) → read_file(/README.md)` |
| 8 | `list_files(/.) → find_files(AGENT.md) → list_files(/frontends)` |
| 6 | `list_files(/.) → find_files(AGENT.md) → read_file(/README.md)` |
| 3 | `list_files(/.) → find_files(AGENT.md) → list_files(/rust)` |
| 3 | `list_files(/.) → read_file(/README.md) → list_files(/frontends)` |
| 2 | `list_files(/.) → find_files(AGENT.md) → read_file(/backlog.md)` |
| 2 | `list_files(/.) → find_files(AGENT.md) → read_file(/rust-migration.md)` |

This follows the system prompt instruction ("look for README, AGENT.md, CLAUDE.md, or similar"). Pre-loading `README.md` and `.omega/system-prompt-append.md` (if it existed) would save these 2–3 opening tool calls in every genuinely-new Omega session.

### 3. Most-read files in orientation prefix (combined corpus, first 8 tool calls)

| Normalised path | Count | % of 483 | Notes |
|---|---:|---:|---|
| `/.` (list_files) | 115 | 23.8 % | Omega repo root listing |
| `/app` (list_files) | 70 | 14.5 % | Harbor task directory (shown as `/` in normalised reports — see §Harbor note below) |
| `/README.md` | 39 | 8.1 % | Top read file |
| `/frontends` | 36 | 7.5 % | Web frontend directory |
| `/rust-migration.md` | 34 | 7.0 % | Ongoing migration notes |
| `/rust/crates/…` | 21 | 4.3 % | Rust workspace |
| `/frontends/leptos` | 15 | 3.1 % | Leptos rewrite |
| `/rust` | 14 | 2.9 % | |
| `/.omega/sessions/…` | 13 | 2.7 % | Session-inspection tasks |
| `/backlog/schema-8.md` | 12 | 2.5 % | Active workstream |
| `/backlog.md` | 11 | 2.3 % | Backlog index |
| `/Justfile` | 7 | 1.4 % | Task runner |

These are mostly Omega-specific files — not generalisable across projects. `README.md` is the only universally-applicable target.

### 4. No prompt type → orientation prefix mapping is reliable

Within every prompt-type class the variance is high: even the most common 3-call sequence for a given type appears in only 2–3 out of 5–48 sessions in that class. The agent's behaviour depends more on what the first tool result reveals than on the prompt text. Concrete examples (from §5 of the analyzer output):

- **Omega `[other]`, n=48**: top prefix appears 2 times (4 % of class)
- **Omega `[file-analysis]`, n=45**: top prefix appears 1 time (2 % of class)
- **Harbor `[file-analysis]`, n=231**: top non-empty prefix appears 3 times (1.3 % of class)
- **Harbor `[code-writing]`, n=30**: top prefix appears 2 times (7 % of class)

### 5. Sessions with no tool calls

| Corpus | No-tool sessions |
|---|---|
| Omega  | 3 / 174 (1.7 %) |
| Harbor | 7 / 309 (2.3 %) |

Omega's no-tool sessions are short conversational exchanges (mostly the `meta/ping` cluster — Carsten's manual liveness checks). Harbor's are tasks where the agent answered from the prompt alone without needing the environment.

---

## Context caching, forking, and subagents

*(Discussion 2026-05-16)*

### System prompt vs. early context — what actually matters

The system prompt is immutable for a session, always present, and carries operator-level authority. Early context (pre-injected before the first human turn) is functionally append-only for the same reasons: removing a message mid-session invalidates every downstream cache breakpoint, making routine shortening too expensive.

The Anthropic cache is **content-addressed, not session-scoped**: any request presenting the same prefix hash hits the same cache entry regardless of which session it comes from. Context "forks" are therefore possible in principle — two message arrays sharing a cached prefix can diverge independently after it. But forking within a single sequential session **requires shortening** as a prerequisite: going from AB back to A before adding C is exactly the same operation as shortening. The economics are only acceptable if a cache breakpoint was placed exactly at the truncation boundary in advance, which requires the application to track scope boundaries at injection time. Treat the messages array as append-only; every injection is a commitment for the life of the session.

### Subagents dissolve the forking problem

A subagent is an agent invocation in a **completely separate context window**. It receives a task as its first human turn, runs its own tool calls, and returns its final output as a single string — which lands in the parent's context as a tool result. The parent sees only that string; the subagent's internal tool calls, file reads, and reasoning never touch the parent's messages array.

All three surveyed agents implement this: forgecode (`Conversation` object), opencode (`Session` with `parentID`; also supports `task_id` to resume a prior subagent session), pi-mono (spawned `pi` subprocess with single/parallel/chain modes).

This resolves the contamination problem directly: instead of injecting subfolder-specific context into the parent and later trying to trim it, dispatch a subagent for the subfolder work. The parent context stays clean; the subfolder's internal work lives in the subagent's isolated window and disappears when it finishes. Subagents are deferred for detailed planning to a later session.

### AGENTS.md injection — decision

All three surveyed agents pre-load project instruction files before the first agent turn. Omega is the only one that makes the agent discover them via tool calls. The decision: standardise on auto-loading. Root `AGENTS.md` → system prompt block (behavioural authority, always relevant, zero tool cost). Nested `AGENTS.md` files → on-demand via tool call when the agent enters that subtree (matches opencode's model). Implementation details deferred to next session.

### Initial directory listing and workspace summary — narrow dismissal

Analysis shows ~86 % of Omega sessions open with at least one `list_files` or orientation tool call. Pre-loading a shallow root listing would fully replace the first call in ~14 % of sessions; the remaining ~52 % still drill deeper afterward for task-specific reasons. AGENTS.md injection handles the semantic part of orientation (what the project is, its conventions) which is what the agent actually needs from those early calls.

Forgecode injects a `<workspace_extensions>` block derived from `git ls-files`: a compact file-type distribution (`.rs: 415 files (50%)`). Appealing because it is tiny, uses git scope to exclude artefacts, and fails gracefully on non-repo directories. However: **models do not reach for `git ls-files` when orienting unprompted** — they reach for `list_files`, `find_files`, and `read_file(README.md)`. Injecting a statistical summary they would not seek is answering a question they are not asking. With AGENTS.md injection covering the semantic equivalent, the workspace summary is dismissed as marginal for now.

### Harbor setup — the `/` listing was a normalisation artefact

The orientation analysis reported `list_files('/') × 70` for Harbor sessions labelled "Harbor container root." This was an artefact of `_strip_cwd()` in `analyze_orientation.py`, which strips `/app` from all paths for comparability. Every one of those 70 was actually `list_files('/app')` — the agents were correctly listing the task directory.

In actual Harbor runs: Docker images place task files in `/app`; `omega_agent.py` runs `cd /app 2>/dev/null || true && omega run`; the Rust binary starts with `cwd=/app`; the system prompt says *"Your working directory is /app"*; the agent knows where it is from turn zero. No confusion in practice.

Latent fragility: `2>/dev/null` silently suppresses the `cd` error for any task that doesn't use `/app`. Fix: remove `2>/dev/null` so the error appears in job logs while `|| true` still lets omega start. No sessions are currently failing because of this — low priority.

---

## Conclusions

1. **For Harbor tasks: nothing to pre-load.** The environment is unknown at launch; the prompt is already the full spec. Adding a pre-orientation step would burn tokens without helping. Confirmed by direct measurement: only 1.0 % of Harbor sessions read instruction/README first, 1.3 % within three calls.

2. **For Omega sessions: auto-loading project files is justified.** The `list_files(/.) → find_files(AGENT.md|README*) → read README` pattern is the system prompt's own instruction made manifest. Pre-loading `README.md` (and any project `AGENTS.md`) would save 2–3 tool calls in new sessions.

3. **The prompt-type classification approach is premature.** Variance within classes is too high — top prefixes account for ≤ 7 % of any class. Universal pre-loading of the project manifest (README + any AGENTS.md) is a better lever than selective per-type loading.

4. **Operator control via the file itself, not a flag.** Auto-load by default; the operator's lever is editing or removing `AGENTS.md`, which is per-project by construction. pi-mono's `--no-context-files` opt-out flag was considered and dismissed as premature — easy to add later if a real use-case appears.

5. **No Anthropic lock-in required.** The "memory slot" (array form of `system`) is handled per-provider in the adapter layer; forgecode and pi-mono prove the logic is provider-agnostic — only the wire format differs per provider.

6. **Do not pursue auto-memory (agent-written memory).** Claude Code allows the agent to write notes to a memory file during a session via a dedicated tool; those notes are injected into the system array at the *next* session's start. The write cannot feed back into the current session — the Anthropic API treats the system array as immutable once the first message is sent. Regardless of this constraint, **the feature is out of scope for Omega**: any cross-session information the agent needs will be handled in a purpose-tailored way by the user (e.g. explicit context files, backlog documents). Automating that is not a goal.

---

## Next steps

### Decided / closed

- **System prompt vs. context framing** — resolved. Context is append-only in practice; subagents are the right tool for isolated subtask work; forking requires shortening and is not worth implementing. See §Context caching, forking, and subagents.
- **Workspace summary (git ls-files)** — dismissed. Models don't reach for this information; AGENTS.md covers the semantic equivalent. See §Initial directory listing and workspace summary.
- **Harbor `/` listing** — was a normalisation artefact, not a real problem. Latent fragility noted; low-priority fix below.
- **Step 2 — Standardised AGENTS.md loading** — done (commit `a726e77`, 2026-05-16). See §Step 2 outcome below.

### Step 2 outcome — Standardised AGENTS.md loading

Implemented automatic pre-loading of project instruction files. Resolved decisions:

- **File name:** `AGENTS.md`. No `CLAUDE.md` awareness. `.omega/system-prompt-append.md` was renamed in place; content moved verbatim (trimming obsolete TS-stack lines is a separate step).
- **Discovery:**
  - Repo file: walk up from `cwd` to the git root, load `<root>/AGENTS.md` if present.
  - Global file: `$XDG_CONFIG_HOME/omega/AGENTS.md` (default `~/.config/omega/AGENTS.md` when XDG is unset).
  - No opt-out flag — always on. The pi-mono `--no-context-files` precedent was considered and rejected for now; can be added later if a use-case appears.
- **Injection point:** separate Anthropic system content blocks (opencode model), four in fixed order: `[1]` core prompt (unheadered, static), `[2]` runtime context (cwd + max_output_tokens, moved out of the core prompt under a `## Runtime context` header), `[3]` global AGENTS.md, `[4]` repo AGENTS.md. Each non-empty AGENTS.md block is prefixed `Instructions from: <path>` so the source is legible to the model. A single `cache_control` marker sits at the tail of block `[4]`, leaving three breakpoints free for the messages array. Ollama receives the same blocks joined with `\n\n` — it has no cache_control concept.
- **Scope:** root only. Tier C (nested AGENTS.md, on-demand attachment when the agent enters a subtree) is designed but not implemented — deferred to a follow-up step alongside subagents.
- **Harbor behaviour:** unchanged in spirit. Harbor containers don't ship an `AGENTS.md` in `/app` and have no global config dir, so discovery yields zero files and the prompt is core + runtime only. No special-casing needed.

Architecture bundled in the same PR: discovery now lives inside `omega-agent::system_prompt`. The caller (CLI or server router) hands the agent only a `cwd`; the agent discovers and loads. `AgentConfig.system_prompt_append` is gone, which makes the previous server-passes-`None` bug — the root cause of the "Omega forgot to commit" symptom — structurally inexpressible. CLI and server now traverse one code path through `Agent::init()`. Side-effect: the CLI's `--effort` flag now actually controls the agent (it previously only labelled the SessionStarted event).

`SessionStarted.system_prompt` remains a single `String` on the wire; the four blocks are joined with `\n\n` for the archive so existing frontend code is untouched while AGENTS.md content is now visible in archived sessions. One `eprintln!` line per discovered AGENTS.md (`AGENTS.md: loaded <path>`) appears at session start, or `AGENTS.md: not found in repo` when neither tier yields a file.

### Next step 3 — Subagents (future session)

Design and implement subagent support. Architecture is understood: separate context window, task as first human turn, result as tool-output string in parent. opencode's `task_id` resumability is an interesting optional extension. Tier C nested-AGENTS.md attachment should be folded into this design — the natural fit is "subagent for subtree X is launched with `<subtree>/AGENTS.md` pre-loaded."

---

## Changelog

- **2026-05-16 (Step 2 landed):** Standardised AGENTS.md loading shipped (commit `a726e77`). Repo-root and `$XDG_CONFIG_HOME/omega` `AGENTS.md` files are discovered and attached as separate cacheable system blocks. `AgentConfig.system_prompt_append` removed; CLI and server now share one code path through `Agent::init()`, eliminating the server-passes-`None` bug. Tier C (nested AGENTS.md) deferred to the subagents work. Details in §Step 2 outcome.
- **2026-05-16 (revised):** Corpus reduced from 765 → 174 Omega sessions after archiving 678 TypeScript-era and mock-polluted sessions to `.omega/sessions-archive-ts/`. The `sleep 10` finding (formerly §1) is removed — it was an artifact of TS-era mock fixtures, not a real Omega behaviour. Added methodology note documenting the t=0 cutoff (2026-05-02 23:38) and the Rust-clean filter now applied by `analyze_orientation.py`. All other findings, conclusions, and open questions are unchanged or only had their counts refreshed against the clean corpus; the orientation-pattern picture is the same shape as before, just sharper.
- **2026-05-16 (discussion session):** Added §Context caching, forking, and subagents covering: append-only context economics, why forking requires shortening (they are the same operation), subagent architecture as the practical alternative to forking, AGENTS.md placement decision, workspace-summary dismissal, and the Harbor normalisation-artefact finding. Corrected `/` → `/app` in the Harbor listing description throughout. Open questions section replaced with three concrete next steps.
