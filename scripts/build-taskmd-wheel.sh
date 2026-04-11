#!/usr/bin/env bash
# Build the taskmd wheel from source (one-time per machine) and place it
# in .taskmd-wheel/ so dev.py can use it via find-links without rebuilding.
# Run this once after cloning: ./scripts/build-taskmd-wheel.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WHEEL_DIR="$REPO_ROOT/.taskmd-wheel"
TASKMD_REPO="https://github.com/scottopell/taskmd.git"
TASKMD_REF="HEAD"

echo "Building taskmd wheel → $WHEEL_DIR"
mkdir -p "$WHEEL_DIR"

# Build via uv + pip wheel (avoids the maturin stdout-capture bug in uv run --script)
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

cd "$TMPDIR"
git clone --depth 1 "$TASKMD_REPO" taskmd
cd taskmd

uv pip wheel --wheel-dir "$WHEEL_DIR" . 2>&1 | tail -5

echo "Done. Wheel in $WHEEL_DIR:"
ls "$WHEEL_DIR"
