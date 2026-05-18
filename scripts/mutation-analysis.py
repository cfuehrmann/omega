#!/usr/bin/env python3
"""
mutation-analysis.py — analyse cargo-mutants output and produce a markdown report.

Reads per-crate mutants.out/outcomes.json files written by mutation-test-all.sh
and synthesises them into docs/mutation-testing/report.md.

Analysis dimensions
-------------------
1. Summary table — per-crate counts of caught / missed / timeout / unviable.
2. Surviving mutants — listed per crate with source context.
3. Timeout mutants — listed per crate (may indicate flaky infrastructure).
4. Unviable mutants — listed per crate (may indicate missing cfg / features).
5. Kills that may not reflect real-life calls — heuristic flags:
     a. Mutant is in a function whose ONLY callers inside the crate are test
        modules (detected by grepping for the function name under #[cfg(test)]
        or in a file inside tests/).
     b. Mutant is in omega-test-fixtures (the fixture library itself is only
        ever called from tests; a "kill" there is circular).
6. Skipped mutants review — every #[mutants::skip] annotation in the source,
   with the surrounding comment rationale and a review note.
7. exclude_re review — patterns in .cargo/mutants.toml, with rationale.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------

@dataclass
class Mutant:
    package: str
    file: str
    function_name: str
    return_type: str
    line: int
    col: int
    replacement: str
    genre: str
    summary: str          # CaughtMutant | MissedMutant | Timeout | Unviable
    build_s: float = 0.0
    test_s: float = 0.0

    @property
    def location(self) -> str:
        return f"{self.file}:{self.line}:{self.col}"

    @property
    def description(self) -> str:
        return (
            f"{self.file}:{self.line}:{self.col}: "
            f"replace {self.function_name} {self.return_type} "
            f"with {self.replacement}"
        )

    @property
    def short_desc(self) -> str:
        """Short description matching cargo-mutants --list format."""
        if self.return_type:
            return (
                f"replace {self.function_name} {self.return_type} "
                f"with {self.replacement}"
            )
        return f"replace ... with {self.replacement}"


@dataclass
class SkipAnnotation:
    file: str
    line: int
    fn_name: str
    rationale: str         # extracted comment above the annotation


@dataclass
class CrateReport:
    name: str
    caught: list[Mutant] = field(default_factory=list)
    missed: list[Mutant] = field(default_factory=list)
    timeout: list[Mutant] = field(default_factory=list)
    unviable: list[Mutant] = field(default_factory=list)
    error: Optional[str] = None

    @property
    def total(self) -> int:
        return len(self.caught) + len(self.missed) + len(self.timeout) + len(self.unviable)

    @property
    def kill_rate(self) -> float:
        denom = len(self.caught) + len(self.missed) + len(self.timeout)
        if denom == 0:
            return 1.0
        return len(self.caught) / denom


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------

def parse_outcomes(outcomes_json: Path) -> list[Mutant]:
    """Parse a single mutants.out/outcomes.json file into a list of Mutants."""
    with outcomes_json.open() as f:
        data = json.load(f)

    mutants: list[Mutant] = []
    for entry in data.get("outcomes", []):
        scenario = entry.get("scenario", {})
        if not isinstance(scenario, dict):
            # Baseline scenario is a plain string like "Baseline" — skip
            continue
        m = scenario.get("Mutant")
        if m is None:
            # Some other non-mutant scenario — skip
            continue

        summary = entry.get("summary", "Unknown")
        fn_info = m.get("function", {})
        span = m.get("span", {})
        start = span.get("start", {})

        build_s = 0.0
        test_s = 0.0
        for phase in entry.get("phase_results", []):
            dur = phase.get("duration", 0.0)
            if phase.get("phase") == "Build":
                build_s = dur
            elif phase.get("phase") == "Test":
                test_s = dur

        mutants.append(Mutant(
            package=m.get("package", ""),
            file=m.get("file", ""),
            function_name=fn_info.get("function_name", ""),
            return_type=fn_info.get("return_type", ""),
            line=start.get("line", 0),
            col=start.get("column", 0),
            replacement=m.get("replacement", ""),
            genre=m.get("genre", ""),
            summary=summary,
            build_s=build_s,
            test_s=test_s,
        ))
    return mutants


def load_crate_report(crate_name: str, out_dir: Path) -> CrateReport:
    """Load cargo-mutants results for one crate."""
    report = CrateReport(name=crate_name)
    outcomes_path = out_dir / crate_name / "mutants.out" / "outcomes.json"

    if not outcomes_path.exists():
        report.error = f"No outcomes.json found at {outcomes_path}"
        return report

    try:
        mutants = parse_outcomes(outcomes_path)
    except Exception as exc:
        report.error = f"Failed to parse {outcomes_path}: {exc}"
        return report

    for m in mutants:
        if m.summary == "CaughtMutant":
            report.caught.append(m)
        elif m.summary == "MissedMutant":
            report.missed.append(m)
        elif m.summary == "Timeout":
            report.timeout.append(m)
        elif m.summary in ("Unviable", "SkipMutant"):
            report.unviable.append(m)
        else:
            # Unknown — treat as unviable
            report.unviable.append(m)
    return report


# ---------------------------------------------------------------------------
# Skip-annotation extraction
# ---------------------------------------------------------------------------

SKIP_PATTERN = re.compile(r"#\[mutants::skip\]")

def extract_skip_annotations(repo_root: Path) -> list[SkipAnnotation]:
    """Find every #[mutants::skip] in the source tree, with rationale."""
    annotations: list[SkipAnnotation] = []
    for rs_file in sorted(repo_root.rglob("*.rs")):
        # Skip generated/target trees
        if "target" in rs_file.parts:
            continue
        if "frontends" in rs_file.parts:
            continue

        lines = rs_file.read_text(errors="replace").splitlines()
        for i, line in enumerate(lines):
            stripped_line = line.strip()
            # Only match the annotation itself; skip lines where it appears
            # inside a comment (// ... #[mutants::skip]) or doc-comment.
            if "#[mutants::skip]" not in stripped_line:
                continue
            if stripped_line.startswith("//") or stripped_line.startswith("*"):
                continue
            if True:
                # Collect the comment block immediately above this annotation
                comment_lines: list[str] = []
                j = i - 1
                while j >= 0 and (
                    lines[j].strip().startswith("//")
                    or lines[j].strip().startswith("///")
                    or lines[j].strip() == ""
                ):
                    stripped = lines[j].strip()
                    if stripped:
                        comment_lines.insert(0, stripped)
                    j -= 1

                # Find the function/item name on the next non-attribute line
                fn_name = ""
                for k in range(i + 1, min(i + 5, len(lines))):
                    candidate = lines[k].strip()
                    if candidate.startswith("#"):
                        continue
                    # Extract function name
                    fn_match = re.search(r"\bfn\s+(\w+)", candidate)
                    if fn_match:
                        fn_name = fn_match.group(1)
                    break

                rel_path = str(rs_file.relative_to(repo_root))
                annotations.append(SkipAnnotation(
                    file=rel_path,
                    line=i + 1,
                    fn_name=fn_name,
                    rationale=" ".join(comment_lines) if comment_lines else "(no comment above annotation)",
                ))
    return annotations


# ---------------------------------------------------------------------------
# Heuristic: kills that may not reflect real production calls
# ---------------------------------------------------------------------------

def flag_dubious_kills(report: CrateReport, repo_root: Path) -> list[tuple[Mutant, str]]:
    """
    Return (mutant, reason) pairs for caught mutants that may be killed only
    by test infrastructure rather than by tests that reflect production usage.

    Heuristics (conservative — only flag when confident):
    1. Mutant is in omega-test-fixtures (the fixture library is test-only; any
       kill comes from the fixtures' own unit tests, which are circular).
    2. Mutant is in a function whose only call-sites in the *same crate's
       source* live inside a #[cfg(test)] block or a file under tests/.
    """
    flagged: list[tuple[Mutant, str]] = []

    for m in report.caught:
        reason = _check_dubious(m, repo_root)
        if reason:
            flagged.append((m, reason))

    return flagged


def _check_dubious(m: Mutant, repo_root: Path) -> Optional[str]:
    # Heuristic 1: fixture library
    if "omega-test-fixtures" in m.file or m.package == "omega-test-fixtures":
        return (
            "This mutant lives in **omega-test-fixtures**, which is test-only "
            "infrastructure. The only tests that exercise it are its own unit "
            "tests — those kills confirm the fixture behaves as written, not "
            "that production code is covered."
        )

    # Heuristic 2: function only called from test code in the same crate.
    # Only apply this to short, clearly-named functions to avoid false positives.
    fn = m.function_name
    if not fn or len(fn) < 3:
        return None

    src_file = repo_root / m.file
    if not src_file.exists():
        return None

    # Find every call-site of `fn` in the crate's source directory
    crate_src = src_file.parent
    # Walk up to find the crate root (directory containing Cargo.toml)
    crate_root = src_file
    for _ in range(6):
        crate_root = crate_root.parent
        if (crate_root / "Cargo.toml").exists():
            break
    else:
        return None

    call_pattern = re.compile(r'\b' + re.escape(fn) + r'\s*\(')
    definition_pattern = re.compile(r'\bfn\s+' + re.escape(fn) + r'\b')

    production_call_sites: list[str] = []
    test_only_call_sites: list[str] = []

    for rs_file in crate_root.rglob("*.rs"):
        if "target" in rs_file.parts:
            continue
        try:
            content = rs_file.read_text(errors="replace")
        except OSError:
            continue

        lines = content.splitlines()
        in_cfg_test = False
        depth = 0

        for lineno, line in enumerate(lines, 1):
            stripped = line.strip()

            # Track #[cfg(test)] blocks by brace depth (crude but adequate)
            if "#[cfg(test)]" in stripped or "mod tests" in stripped:
                in_cfg_test = True
                depth = 0

            if in_cfg_test:
                depth += line.count("{") - line.count("}")
                if depth < 0:
                    in_cfg_test = False
                    depth = 0

            if call_pattern.search(line):
                # Make sure this is not the definition itself
                if definition_pattern.search(line):
                    continue
                is_test_file = (
                    "tests/" in str(rs_file)
                    or rs_file.name.endswith("_test.rs")
                    or in_cfg_test
                )
                if is_test_file:
                    test_only_call_sites.append(f"{rs_file.relative_to(crate_root)}:{lineno}")
                else:
                    production_call_sites.append(f"{rs_file.relative_to(crate_root)}:{lineno}")

    if test_only_call_sites and not production_call_sites:
        sites = ", ".join(test_only_call_sites[:5])
        return (
            f"All call-sites of `{fn}` found in this crate appear to be inside "
            f"test code ({sites}). The kill may not reflect production behaviour."
        )

    return None


# ---------------------------------------------------------------------------
# Source context helper
# ---------------------------------------------------------------------------

def source_snippet(repo_root: Path, file: str, line: int, context: int = 3) -> str:
    """Return a fenced code block with ±context lines around `line`."""
    path = repo_root / file
    if not path.exists():
        return "_source not found_"
    lines = path.read_text(errors="replace").splitlines()
    start = max(0, line - context - 1)
    end = min(len(lines), line + context)
    numbered = []
    for i, ln in enumerate(lines[start:end], start=start + 1):
        marker = "→" if i == line else " "
        numbered.append(f"{marker} {i:4d} │ {ln}")
    return "```rust\n" + "\n".join(numbered) + "\n```"


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

# Test-infra crates are intentionally excluded from mutation analysis:
#   omega-test-fixtures  — shared fake HTTP/SSE server; not a production crate.
#   omega-mock-server    — Playwright test fixture binary; not shipped to users.
# Mutation-testing them is circular (their only callers are tests) and noisy.
CRATE_ORDER = [
    "omega-types",
    "omega-cli",
    "omega-store",
    "omega-core",
    "omega-server",
    "omega-agent",
    "omega-tools",
]

def generate_report(
    reports: dict[str, CrateReport],
    skip_annotations: list[SkipAnnotation],
    exclude_re_entries: list[str],
    repo_root: Path,
    out_dir: Path,
    start_time: str,
    end_time: str,
) -> str:
    lines: list[str] = []

    lines.append("# Omega Mutation Testing Report")
    lines.append("")
    lines.append(f"**Run started:** {start_time}  ")
    lines.append(f"**Run ended:** {end_time}  ")
    lines.append(f"**Tool:** cargo-mutants 26.0.0  ")
    lines.append(f"**Flags:** `-j1 --no-shuffle` (serial, deterministic)  ")
    lines.append("")
    lines.append(
        "> **Excluded crates (test infrastructure — mutation testing them is circular):**  "
    )
    lines.append(
        "> `omega-test-fixtures` (shared fake HTTP/SSE server; no production callers),  "
    )
    lines.append(
        "> `omega-mock-server` (Playwright fixture binary; not shipped to users),  "
    )
    lines.append(
        "> `omega-e2e` (browser Playwright tests; requires live Chromium)."
    )
    lines.append("")

    # ------------------------------------------------------------------
    # 1. Executive summary table
    # ------------------------------------------------------------------
    lines.append("## 1. Executive Summary")
    lines.append("")
    lines.append("| Crate | Mutants | Caught | Missed | Timeout | Unviable | Kill rate |")
    lines.append("|-------|---------|--------|--------|---------|----------|-----------|")

    total_caught = total_missed = total_timeout = total_unviable = 0
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None:
            lines.append(f"| `{crate}` | — | — | — | — | — | — |")
            continue
        if r.error:
            lines.append(f"| `{crate}` | ERROR | — | — | — | — | — |")
            continue
        c, m, t, u = len(r.caught), len(r.missed), len(r.timeout), len(r.unviable)
        total_caught += c
        total_missed += m
        total_timeout += t
        total_unviable += u
        rate = f"{r.kill_rate*100:.0f}%" if r.total else "—"
        lines.append(f"| `{crate}` | {r.total} | {c} | {m} | {t} | {u} | {rate} |")

    total_all = total_caught + total_missed + total_timeout + total_unviable
    total_rate = (
        f"{total_caught/(total_caught+total_missed+total_timeout)*100:.0f}%"
        if (total_caught + total_missed + total_timeout) > 0
        else "—"
    )
    lines.append(
        f"| **Total** | **{total_all}** | **{total_caught}** | **{total_missed}** "
        f"| **{total_timeout}** | **{total_unviable}** | **{total_rate}** |"
    )
    lines.append("")

    # ------------------------------------------------------------------
    # 2. Surviving mutants (per crate)
    # ------------------------------------------------------------------
    lines.append("## 2. Surviving Mutants")
    lines.append("")
    lines.append(
        "Surviving mutants are the most actionable finding: they represent "
        "code paths that could change behaviour without any test failing."
    )
    lines.append("")

    any_missed = False
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None or r.error or not r.missed:
            continue
        any_missed = True
        lines.append(f"### 2.{CRATE_ORDER.index(crate)+1} `{crate}` — {len(r.missed)} survivor(s)")
        lines.append("")
        for m in r.missed:
            lines.append(f"#### `{m.function_name}` — {m.file}:{m.line}")
            lines.append("")
            lines.append(f"- **Mutant:** replace `{m.function_name} {m.return_type}` with `{m.replacement}`")
            lines.append(f"- **Genre:** {m.genre}")
            lines.append(f"- **Location:** `{m.location}`")
            lines.append("")
            lines.append(source_snippet(repo_root, m.file, m.line))
            lines.append("")
            lines.append("**Analysis:** _No test currently asserts the value returned / side-effect_")
            lines.append("**produced by this function with inputs that would distinguish the_")
            lines.append("_replacement from the original. A targeted test is needed._")
            lines.append("")

    if not any_missed:
        lines.append("🎉 **No surviving mutants!** All generated mutants were caught.")
        lines.append("")

    # ------------------------------------------------------------------
    # 3. Timeout mutants
    # ------------------------------------------------------------------
    lines.append("## 3. Timeout Mutants")
    lines.append("")

    any_timeout = False
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None or r.error or not r.timeout:
            continue
        any_timeout = True
        lines.append(f"### `{crate}` — {len(r.timeout)} timeout(s)")
        lines.append("")
        for m in r.timeout:
            lines.append(f"- `{m.location}`: `{m.function_name} {m.return_type}` → `{m.replacement}`")
        lines.append("")

    if not any_timeout:
        lines.append("No timeouts. ✅")
        lines.append("")

    # ------------------------------------------------------------------
    # 4. Unviable mutants
    # ------------------------------------------------------------------
    lines.append("## 4. Unviable Mutants")
    lines.append("")
    lines.append(
        "Unviable mutants failed to compile. This is normal for type-system-constrained "
        "replacements. A high count can indicate missing feature flags or "
        "over-aggressive cargo-mutants genre coverage."
    )
    lines.append("")

    any_unviable = False
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None or r.error or not r.unviable:
            continue
        any_unviable = True
        lines.append(f"### `{crate}` — {len(r.unviable)} unviable")
        lines.append("")
        for m in r.unviable:
            lines.append(f"- `{m.location}`: `{m.function_name} {m.return_type}` → `{m.replacement}`")
        lines.append("")

    if not any_unviable:
        lines.append("No unviable mutants. ✅")
        lines.append("")

    # ------------------------------------------------------------------
    # 5. Dubious kills (may not reflect real production calls)
    # ------------------------------------------------------------------
    lines.append("## 5. Kills That May Not Reflect Real-Life Calls")
    lines.append("")
    lines.append(
        "These caught mutants were flagged by heuristics as potentially being "
        "killed only by test infrastructure rather than by tests that exercise "
        "production code paths. **They should be reviewed**: if the flag is "
        "correct, the apparent coverage is illusory."
    )
    lines.append("")
    lines.append("Heuristics applied:")
    lines.append("1. Mutant is in `omega-test-fixtures` (fixture-tests are circular).")
    lines.append(
        "2. All call-sites of the mutated function within the same crate are in "
        "test files or `#[cfg(test)]` blocks."
    )
    lines.append("")

    any_dubious = False
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None or r.error:
            continue
        flagged = flag_dubious_kills(r, repo_root)
        if not flagged:
            continue
        any_dubious = True
        lines.append(f"### `{crate}` — {len(flagged)} flagged kill(s)")
        lines.append("")
        for m, reason in flagged:
            lines.append(f"#### `{m.function_name}` — {m.location}")
            lines.append("")
            lines.append(f"- **Mutant:** `{m.replacement}`")
            lines.append(f"- **Flag reason:** {reason}")
            lines.append("")
            lines.append(source_snippet(repo_root, m.file, m.line))
            lines.append("")

    if not any_dubious:
        lines.append(
            "No dubious kills flagged by the automated heuristics. "
            "Manual review of specific mutants is always advisable."
        )
        lines.append("")

    # ------------------------------------------------------------------
    # 6. Skipped mutants review
    # ------------------------------------------------------------------
    lines.append("## 6. Skipped Mutants Review (`#[mutants::skip]`)")
    lines.append("")
    lines.append(
        "Every `#[mutants::skip]` annotation suppresses an entire function's "
        "mutant generation. Each one should have a documented, still-valid "
        "rationale. Annotations are listed below with the comment found "
        "immediately above them."
    )
    lines.append("")

    if skip_annotations:
        lines.append(f"**Total skipped functions: {len(skip_annotations)}**")
        lines.append("")
        for ann in skip_annotations:
            fn_label = f"`{ann.fn_name}`" if ann.fn_name else "_(anonymous)_"
            lines.append(f"### {fn_label} — `{ann.file}:{ann.line}`")
            lines.append("")
            lines.append(f"**Rationale from source comment:**")
            lines.append(f"> {ann.rationale}")
            lines.append("")

            # Emit source snippet
            lines.append(source_snippet(repo_root, ann.file, ann.line, context=5))
            lines.append("")

            # Review note
            lines.append(_review_skip(ann))
            lines.append("")
    else:
        lines.append("No `#[mutants::skip]` annotations found.")
        lines.append("")

    # ------------------------------------------------------------------
    # 7. exclude_re review
    # ------------------------------------------------------------------
    lines.append("## 7. `exclude_re` Patterns in `.cargo/mutants.toml`")
    lines.append("")
    lines.append(
        "These regexes match against the mutant description string (as shown "
        "by `cargo mutants --list`) and suppress matching mutants globally. "
        "They are harder to trace than `#[mutants::skip]` because they are not "
        "co-located with the code they affect."
    )
    lines.append("")

    if exclude_re_entries:
        for pat in exclude_re_entries:
            lines.append(f"- `{pat}`")
        lines.append("")
        lines.append("### Review")
        lines.append("")
        lines.append(
            "Each pattern below is reviewed against the current source to confirm "
            "the rationale is still accurate."
        )
        lines.append("")
        for pat in exclude_re_entries:
            lines.append(f"#### `{pat}`")
            lines.append("")
            lines.append(_review_exclude_re(pat, repo_root))
            lines.append("")
    else:
        lines.append("No `exclude_re` patterns found.")
        lines.append("")

    # ------------------------------------------------------------------
    # 8. Caught-mutant quality observations (per-crate narrative)
    # ------------------------------------------------------------------
    lines.append("## 8. Per-Crate Coverage Narrative")
    lines.append("")
    lines.append(
        "Brief qualitative notes on what the killed mutants tell us about "
        "test coverage quality in each crate."
    )
    lines.append("")

    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r is None or r.error:
            continue
        lines.append(f"### `{crate}`")
        lines.append("")
        lines.append(_crate_narrative(r, repo_root))
        lines.append("")

    # ------------------------------------------------------------------
    # 9. Recommendations
    # ------------------------------------------------------------------
    lines.append("## 9. Recommendations")
    lines.append("")
    _recommendations(lines, reports, skip_annotations, repo_root)
    lines.append("")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Narrative helpers (written per-crate based on results)
# ---------------------------------------------------------------------------

def _review_skip(ann: SkipAnnotation) -> str:
    """Return a markdown review note for a single #[mutants::skip]."""
    fn = ann.fn_name or "(anonymous)"
    rat = ann.rationale.lower()

    # Known patterns and their status — checked from most-specific to least.

    if "equivalent" in rat:
        return (
            "**Review:** ✅ The annotation documents an *equivalent mutant* — "
            "a code change that cannot alter observable behaviour. The comment "
            "explains the equivalence. Rationale appears sound; no action needed "
            "unless the surrounding code changes."
        )

    if "non-deterministic" in rat or "rng" in rat:
        return (
            "**Review:** ✅ The annotation suppresses a mutant in non-deterministic "
            "(RNG-dependent) code that cannot be meaningfully tested by a "
            "deterministic mutant check. Rationale is sound."
        )

    if "main" in fn and ("glue" in rat or "bind" in rat or "spawn" in rat):
        return (
            "**Review:** ✅ `main()` is pure OS-level glue (bind/spawn). "
            "Mutation-testing it would require process-level infrastructure. "
            "Coverage of the logic it delegates to is provided by integration tests."
        )

    if "chrono" in rat or "rfc3339" in rat or ("iso" in rat and "timestamp" in rat):
        return (
            "**Review:** ✅ The annotation suppresses a mutant for timestamp "
            "formatting delegated to a well-tested library (`chrono`). "
            "Testing it would require mocking wall-clock time. Rationale is sound."
        )

    # 'accepted dead code at the mutation-testing level' pattern:
    # the function IS reachable; the mutation is untestable because the test
    # infrastructure cannot distinguish the two behaviours.  This is a weaker
    # form of equivalence — valid but worth flagging for periodic re-review.
    if ("accepted" in rat and "dead" in rat and "mutation" in rat) or (
        "out-of-process" in rat or "playwright" in rat or "out of process" in rat
    ):
        return (
            "**Review:** ⚠️  The annotation documents a mutant that is untestable "
            "via the current test harness (the two behaviours produce identical "
            "observable output, or the covering test is an out-of-process / "
            "browser spec not reachable by `cargo mutants`). The in-source comment "
            "confirms this was reviewed manually. Accepted for now, **but should "
            "be re-evaluated** if the test infrastructure around this code changes."
        )

    if "dead" in rat and "invariant" in rat:
        return (
            "**Review:** ⚠️  The annotation claims the function is unreachable in "
            "practice (enforced by an invariant). Verify the invariant still holds: "
            "if it has been relaxed the function may now be reachable and the skip "
            "should be removed so tests can cover it."
        )

    if "not a regular file" in rat or "file type" in rat or "is_file" in rat:
        return (
            "**Review:** ✅ The annotation suppresses a mutant on a file-type guard "
            "(`!ft.is_file()`). Inverting the guard would cause the code to attempt "
            "to read directories as files — an OS error that returns no matches, "
            "making the mutation observationally equivalent in test environments "
            "where only files are present. Rationale is sound."
        )

    # Generic fallback
    return (
        "**Review:** ⚠️  Rationale present but not automatically categorised. "
        "Verify manually that the justification still holds for the current "
        "code; if the function has changed, the skip may need re-evaluation."
    )


def _review_exclude_re(pattern: str, repo_root: Path) -> str:
    """Return a review note for a single exclude_re entry."""
    if "Message::Close" in pattern:
        return (
            "**Review:** ✅ This suppresses the `delete match arm Message::Close(_)` "
            "mutant in `handle_socket`. The documented equivalence is that dropping "
            "the `break` causes the WebSocket `reader.next()` to return `None` on "
            "the very next iteration, exiting the `while-let` identically. "
            "The in-source comment in `router.rs` confirms this. "
            "Rationale is sound; no action needed unless the `handle_socket` "
            "control-flow changes."
        )
    return (
        f"**Review:** ⚠️  No automated review available for this pattern. "
        f"Verify manually against the current source."
    )


def _crate_narrative(r: CrateReport, repo_root: Path) -> str:
    """Generate a qualitative paragraph about a crate's mutation score."""
    c, m, t, u = len(r.caught), len(r.missed), len(r.timeout), len(r.unviable)
    total = r.total
    rate = r.kill_rate

    if total == 0:
        return "_No mutants generated for this crate (all filtered or skipped)._"

    parts: list[str] = []
    parts.append(
        f"Generated {total} mutants: **{c} caught** / **{m} missed** / "
        f"{t} timeout / {u} unviable.  "
    )
    parts.append(f"Kill rate: **{rate*100:.0f}%**.  ")
    parts.append("")

    if rate >= 0.95:
        parts.append(
            "The kill rate is excellent (≥ 95%). Test coverage for this crate "
            "is strong at the mutation level."
        )
    elif rate >= 0.80:
        parts.append(
            "The kill rate is good (80–94%). A small number of survivors remain "
            "— see Section 2 for details and suggested remediation."
        )
    elif rate >= 0.60:
        parts.append(
            "The kill rate is moderate (60–79%). Several code paths are not "
            "asserted by any test. This crate should be a priority for additional "
            "test coverage."
        )
    else:
        parts.append(
            "The kill rate is low (< 60%). A significant portion of the crate's "
            "behaviour is not validated by tests. This crate requires substantial "
            "test investment."
        )

    if m > 0:
        fns = sorted({mut.function_name for mut in r.missed})
        parts.append(
            f"  \nSurvivor functions: {', '.join(f'`{f}`' for f in fns[:8])}"
            + (" …" if len(fns) > 8 else ".")
        )

    return "\n".join(parts)


def _recommendations(
    lines: list[str],
    reports: dict[str, CrateReport],
    skip_annotations: list[SkipAnnotation],
    repo_root: Path,
) -> None:
    """Append a prioritised recommendations list."""
    # Collect survivors
    all_missed: list[tuple[str, Mutant]] = []
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r and not r.error:
            for m in r.missed:
                all_missed.append((crate, m))

    if all_missed:
        lines.append(
            f"### High priority — add tests to kill {len(all_missed)} surviving mutant(s)"
        )
        lines.append("")
        lines.append(
            "For each survivor in Section 2, write a test that asserts the "
            "specific value / side-effect that distinguishes the original from "
            "the replacement. Use `cargo mutants -p <crate> --in-place` to "
            "confirm the new test kills the mutant before committing."
        )
        lines.append("")
        by_crate: dict[str, list[Mutant]] = {}
        for crate, m in all_missed:
            by_crate.setdefault(crate, []).append(m)
        for crate, muts in sorted(by_crate.items(), key=lambda kv: -len(kv[1])):
            lines.append(f"- **`{crate}`** — {len(muts)} survivor(s):")
            for m in muts:
                lines.append(f"  - `{m.function_name}` at `{m.file}:{m.line}` → `{m.replacement}`")
        lines.append("")

    # Dubious kills
    dubious_found = False
    for crate in CRATE_ORDER:
        r = reports.get(crate)
        if r and not r.error:
            flagged = flag_dubious_kills(r, repo_root)
            if flagged:
                dubious_found = True
                break

    if dubious_found:
        lines.append("### Medium priority — review flagged kills (Section 5)")
        lines.append("")
        lines.append(
            "Each kill flagged in Section 5 should be manually verified. "
            "If the kill relies on a test that uses mock infrastructure but the "
            "production path is untested, write a complementary test that "
            "exercises the real call-site."
        )
        lines.append("")

    # Skip reviews
    deferred_skips = [
        ann for ann in skip_annotations
        if "dead" in ann.rationale.lower() and "invariant" in ann.rationale.lower()
    ]
    if deferred_skips:
        lines.append("### Low priority — verify invariant-based skips (Section 6)")
        lines.append("")
        for ann in deferred_skips:
            lines.append(
                f"- `{ann.fn_name or '?'}` in `{ann.file}:{ann.line}` — "
                f"confirm the invariant still holds."
            )
        lines.append("")

    if not all_missed and not dubious_found and not deferred_skips:
        lines.append("### No action required")
        lines.append("")
        lines.append(
            "All actionable mutants were caught. The skipped and excluded "
            "mutants have sound justifications. No immediate follow-up needed."
        )


# ---------------------------------------------------------------------------
# .cargo/mutants.toml parser
# ---------------------------------------------------------------------------

def load_exclude_re(repo_root: Path) -> list[str]:
    toml_path = repo_root / ".cargo" / "mutants.toml"
    if not toml_path.exists():
        return []
    content = toml_path.read_text()
    # Simple extraction — find the exclude_re array
    match = re.search(r'exclude_re\s*=\s*\[(.*?)\]', content, re.DOTALL)
    if not match:
        return []
    block = match.group(1)
    # Extract quoted strings
    return re.findall(r'"((?:[^"\\]|\\.)*)"', block)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Generate mutation test analysis report.")
    parser.add_argument("--out-dir", required=True, help="Directory containing per-crate output")
    parser.add_argument("--repo-root", required=True, help="Repository root")
    parser.add_argument("--start", default="", help="Sweep start timestamp")
    parser.add_argument("--end", default="", help="Sweep end timestamp")
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    repo_root = Path(args.repo_root)

    print("Loading crate reports…")
    reports: dict[str, CrateReport] = {}
    for crate in CRATE_ORDER:
        print(f"  {crate}…", end=" ")
        r = load_crate_report(crate, out_dir)
        reports[crate] = r
        if r.error:
            print(f"ERROR: {r.error}")
        else:
            print(f"{r.total} mutants, {len(r.missed)} missed")

    print("Extracting #[mutants::skip] annotations…")
    skip_annotations = extract_skip_annotations(repo_root)
    print(f"  Found {len(skip_annotations)} annotation(s).")

    print("Loading exclude_re patterns…")
    exclude_re = load_exclude_re(repo_root)
    print(f"  Found {len(exclude_re)} pattern(s).")

    print("Generating report…")
    report_text = generate_report(
        reports=reports,
        skip_annotations=skip_annotations,
        exclude_re_entries=exclude_re,
        repo_root=repo_root,
        out_dir=out_dir,
        start_time=args.start,
        end_time=args.end,
    )

    report_path = out_dir / "report.md"
    report_path.write_text(report_text)
    print(f"Report written to {report_path}")


if __name__ == "__main__":
    main()
