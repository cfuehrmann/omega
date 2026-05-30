#!/usr/bin/env bash
# build_release_binary.sh
#
# Build the omega-cli binary inside an ubuntu:20.04 container so the
# resulting ELF links against glibc 2.31 — the lowest glibc version
# present across Terminal-Bench 2 task images.  A host-native build
# uses the host's glibc (currently 2.43 on this machine), causing
# "GLIBC_2.38 not found" errors on 4 of 10 TB2 containers that still
# carry older base images.
#
# Usage:
#   ./bench/build_release_binary.sh [--target-dir DIR]
#
# Options:
#   --target-dir DIR  Cargo target directory (default: target-builder,
#                     relative to the repo root).  Kept separate from
#                     target/ so dev builds and this script never collide.
#
# Prerequisites:
#   docker  — must be on PATH and the current user must be able to run it.
#
# Idempotent: re-running uses cargo's incremental cache in the persisted
# target-builder/ directory (mounted into the container each run).

set -euo pipefail

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------

TARGET_DIR_NAME="target-builder"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target-dir)
            TARGET_DIR_NAME="$2"
            shift 2
            ;;
        --target-dir=*)
            TARGET_DIR_NAME="${1#*=}"
            shift
            ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | grep '^#' | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Locate repo root (one level above bench/ if invoked from bench/, or
# just use the script's own parent).
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$REPO_ROOT/$TARGET_DIR_NAME"
BINARY="$TARGET_DIR/release/omega"

# ---------------------------------------------------------------------------
# Prerequisites check
# ---------------------------------------------------------------------------

if ! command -v docker &>/dev/null; then
    echo "ERROR: 'docker' not found on PATH." >&2
    echo "Install Docker first: https://docs.docker.com/get-docker/" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Build inside ubuntu:20.04 (glibc 2.31)
# ---------------------------------------------------------------------------

echo "=== Building omega-cli inside ubuntu:20.04 (glibc 2.31) ==="
echo "    Repo:       $REPO_ROOT"
echo "    Target dir: $TARGET_DIR"
echo ""

docker run --rm \
    -v "$REPO_ROOT:/work" -w /work \
    -e "CARGO_TARGET_DIR=/work/$TARGET_DIR_NAME" \
    ubuntu:20.04 bash -c '
        set -euo pipefail
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        # Install gcc-10 (not gcc-9): aws-lc-sys refuses gcc-9 due to memcmp bug
        # (https://gcc.gnu.org/bugzilla/show_bug.cgi?id=95189, fixed in gcc-10).
        # gcc-10 and g++-10 are available in the standard focal repos.
        apt-get install -y --no-install-recommends \
            ca-certificates curl build-essential pkg-config libssl-dev git \
            gcc-10 g++-10
        # Point cc/c++ at gcc-10 so the aws-lc-sys build script picks it up.
        update-alternatives --install /usr/bin/gcc gcc /usr/bin/gcc-10 100
        update-alternatives --install /usr/bin/g++ g++ /usr/bin/g++-10 100
        update-alternatives --install /usr/bin/cc  cc  /usr/bin/gcc-10 100
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable
        . "$HOME/.cargo/env"
        cargo build -p omega-cli --release
    '

# ---------------------------------------------------------------------------
# Verify binary exists
# ---------------------------------------------------------------------------

if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Expected binary not found at $BINARY after build." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Check max GLIBC_* version referenced by the binary
# ---------------------------------------------------------------------------

MAX_GLIBC=$(objdump -T "$BINARY" 2>/dev/null \
    | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' \
    | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' \
    | sort -V \
    | tail -1)

if [[ -z "$MAX_GLIBC" ]]; then
    # Try nm --dynamic as a fallback
    MAX_GLIBC=$(nm --dynamic "$BINARY" 2>/dev/null \
        | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' \
        | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' \
        | sort -V \
        | tail -1)
fi

if [[ -z "$MAX_GLIBC" ]]; then
    echo "WARNING: Could not determine max GLIBC version (objdump/nm produced no matches)." >&2
    MAX_GLIBC="unknown"
else
    # Version comparison: fail if max glibc > 2.31
    # We use sort -V and check that 2.31 sorts last (i.e., is >= MAX_GLIBC).
    HIGHEST=$(printf '2.31\n%s\n' "$MAX_GLIBC" | sort -V | tail -1)
    if [[ "$HIGHEST" != "2.31" && "$MAX_GLIBC" != "2.31" ]]; then
        echo "ERROR: Binary requires GLIBC_$MAX_GLIBC > 2.31." >&2
        echo "       The ubuntu:20.04 build should not produce this." >&2
        echo "       Check that the container image was not substituted." >&2
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Print summary
# ---------------------------------------------------------------------------

BINARY_SIZE=$(stat -c%s "$BINARY" 2>/dev/null || stat -f%z "$BINARY" 2>/dev/null || echo "unknown")

# Detect TLS linkage (dynamic vs static)
TLS_NOTE="dynamic via libssl"
if objdump -p "$BINARY" 2>/dev/null | grep -q 'libssl\|libcrypto'; then
    TLS_NOTE="dynamic via libssl"
else
    TLS_NOTE="rustls (no libssl dependency)"
fi

echo ""
echo "Built: $BINARY | size: $BINARY_SIZE | max GLIBC: $MAX_GLIBC | linked TLS: $TLS_NOTE"
