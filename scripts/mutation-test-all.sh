#!/usr/bin/env bash
# mutation-test-all.sh — run cargo-mutants on every workspace crate serially
# and emit a markdown analysis report.
#
# Usage:
#   bash scripts/mutation-test-all.sh [--baseline skip|run]
#
# Output:
#   docs/mutation-testing/<crate>/mutants.out/  — raw cargo-mutants output
#   docs/mutation-testing/<crate>.log           — cargo-mutants stdout+stderr
#   docs/mutation-testing/report.md             — full analysis (written last)
#
# Rationale for -j1:
#   Running one mutant at a time prevents CPU saturation; the host has
#   simultaneous Rust compilation happening in other crates during a workspace
#   test run.  The sweep is intended for overnight execution so wall-clock
#   time is not the constraint.
#
# The `--no-shuffle` flag produces a deterministic ordering within each crate
# so results can be compared across runs.
#
# omega-e2e is excluded globally in .cargo/mutants.toml (browser tests).

set -euo pipefail

BASELINE="${1:-run}"
if [[ "$BASELINE" == "--baseline" ]]; then
    BASELINE="${2:-run}"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$REPO_ROOT/docs/mutation-testing"
TMPDIR_MUTANTS="${HOME}/.cache/cargo-mutants-tmp"

mkdir -p "$TMPDIR_MUTANTS"
mkdir -p "$OUT_DIR"

# Ordered from smallest to largest (by mutant count) so we get early results
# quickly and can inspect the report while heavier crates are still running.
CRATES=(
    omega-types        #   5 mutants
    omega-mock-server  #  15 mutants
    omega-cli          #  20 mutants
    omega-test-fixtures#  31 mutants
    omega-store        #  91 mutants
    omega-core         # 108 mutants
    omega-server       # 110 mutants
    omega-agent        # 175 mutants
    omega-tools        # 275 mutants
)

START_TIME=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "========================================"
echo "  Omega mutation sweep — $START_TIME"
echo "  Baseline: $BASELINE"
echo "  Output:   $OUT_DIR"
echo "========================================"
echo ""

PASS_COUNT=0
FAIL_COUNT=0
FAILED_CRATES=()

for CRATE in "${CRATES[@]}"; do
    # Strip inline comments from the crate name
    CRATE="${CRATE%%#*}"
    CRATE="${CRATE// /}"

    CRATE_OUT="$OUT_DIR/$CRATE"
    CRATE_LOG="$OUT_DIR/${CRATE}.log"

    echo "----------------------------------------"
    echo "  Running: $CRATE"
    echo "  Output:  $CRATE_OUT"
    echo "  Log:     $CRATE_LOG"
    echo "  Start:   $(date -u +"%H:%M:%SZ")"
    echo "----------------------------------------"

    mkdir -p "$CRATE_OUT"

    # Run cargo-mutants; capture all output to the per-crate log.
    # We do NOT use `set -e` here because a non-zero exit from cargo-mutants
    # just means some mutants survived — that is expected and not a script error.
    # Temporarily disable errexit so a non-zero cargo-mutants result
    # (exit 2 = missed mutants, exit 3 = timeouts) does not abort the script.
    # cargo-mutants exit codes (as of 26.x):
    #   0 = all viable mutants caught      -> success
    #   1 = usage / configuration error   -> fatal (abort sweep)
    #   2 = some mutants not caught       -> expected; sweep continues
    #   3 = some tests timed out          -> expected; sweep continues
    set +e
    TMPDIR="$TMPDIR_MUTANTS" cargo mutants \
        --package "$CRATE" \
        --output "$CRATE_OUT" \
        --baseline "$BASELINE" \
        -j1 \
        --no-shuffle \
        --colors never \
        2>&1 | tee "$CRATE_LOG"
    EXIT=${PIPESTATUS[0]}
    set -e
    if [[ $EXIT -eq 0 ]]; then
        echo "  ✅ $CRATE — all mutants caught"
        PASS_COUNT=$((PASS_COUNT + 1))
    elif [[ $EXIT -eq 2 ]]; then
        echo "  ⚠️  $CRATE — completed with missed mutants (exit $EXIT)"
        PASS_COUNT=$((PASS_COUNT + 1))
    elif [[ $EXIT -eq 3 ]]; then
        echo "  ⚠️  $CRATE — completed with timeout mutants (exit $EXIT)"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "  ❌ $CRATE — FAILED (exit $EXIT — usage/config error)"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        FAILED_CRATES+=("$CRATE")
    fi
    echo ""
done

END_TIME=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "========================================"
echo "  Sweep complete — $END_TIME"
echo "  Successful crate runs: $PASS_COUNT"
echo "  Failed crate runs:     $FAIL_COUNT"
if [[ ${#FAILED_CRATES[@]} -gt 0 ]]; then
    echo "  Failed crates: ${FAILED_CRATES[*]}"
fi
echo "========================================"
echo ""

echo "Generating analysis report…"
python3 "$SCRIPT_DIR/mutation-analysis.py" \
    --out-dir "$OUT_DIR" \
    --repo-root "$REPO_ROOT" \
    --start "$START_TIME" \
    --end "$END_TIME"

echo "Report written to: $OUT_DIR/report.md"
