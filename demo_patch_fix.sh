#!/bin/bash
# Demo: Patch Tool Fix Verification
#
# This script demonstrates that the patch tool now correctly handles
# the "overwrite then replace" scenario that was previously failing.

set -e

echo "============================================"
echo "Phoenix IDE Patch Tool Fix Demonstration"
echo "============================================"
echo

# Run the specific integration test
echo "Running integration test: test_overwrite_then_replace_with_filesystem"
echo

cd /home/exedev/phoenix-ide
cargo test test_overwrite_then_replace_with_filesystem --release 2>&1 | grep -E "(running|test.*ok|test.*FAILED|passed|failed)"

echo
echo "============================================"
echo "All property-based tests (6 invariants, 500 cases each):"
echo "============================================"
echo

cargo test --release 2>&1 | grep -E "(test tools::patch::proptests::prop|passed|failed)"

echo
echo "============================================"
echo "Summary"
echo "============================================"
echo
echo "The fix ensures that:"
echo "1. Overwrite creates a file with content X"
echo "2. Replace can find X and replace it with Y"
echo "3. The final file contains Y"
echo
echo "Previously, step 2 would fail with 'old_text not found' because"
echo "the patch planner kept stale content in memory instead of reading"
echo "the actual file content from disk."
echo
echo "The refactor to an Effect/Command pattern isolates all IO in"
echo "executor.rs, making the core planner pure and property-testable."
