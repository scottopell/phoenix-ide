# Add structurally test-aware Rust timing lint

The E2E flake audit prototyped ast-grep rules for real sleeps and unbounded `recv()`/`Notify` waits in Rust tests. The current ast-grep runner cannot reliably relate nested calls to sibling `#[test]` or `#[tokio::test]` attributes, while global rules incorrectly flag production event loops and intentional mock behavior.

After PR #501 ships, evaluate: newer ast-grep relational capabilities, path-scoped invocation, a diff-aware `syn`-based checker, or a narrow Clippy/custom lint. The chosen guard must distinguish test bodies from production and mock behavior, recognize an enclosing `tokio::time::timeout`, support explicit justified behavior-driver exemptions, and include positive/negative fixture tests. Avoid a broad allowlist that turns enforcement into convention.

## Implementation

The structural guard uses ast-grep's Rust parser and JSON byte ranges, then correlates suspect waits with enclosing annotated test functions and `#[cfg(test)]` modules. Byte offsets are compared against UTF-8 source bytes so non-ASCII text before a test cannot shift scope detection.

It rejects new or changed test lines containing:

- `tokio::time::sleep`, `std::thread::sleep`, or `thread::sleep`;
- direct `.recv().await` or `.notified().await` outside an enclosing `tokio::time::timeout`.

Production functions are excluded structurally. A one-line `// test-timing-allow: <reason>` immediately above a wait is available only when elapsed time is itself the behavior being exercised. Empty reasons are rejected.

The repository contains a legacy inventory including intentional delayed protocol fixtures and timing bets owed separate cleanup. Existing `dev.py` check planning owns base selection and resolves one comparison commit; the checker receives that commit, scans both base and working-tree Rust syntax, and compares semantic finding multisets. This catches context-only regressions such as removing a timeout or adding test scope, without failing a legacy finding merely because an unrelated nearby line changed. `--all` reports the complete current inventory for cleanup work.

The checker runs in the existing structural-lint lane and has fixture coverage for annotated tests with stacked attributes, helpers in dedicated test paths and `cfg(test)` scopes, Unicode byte offsets, imported Tokio sleeps, production exclusion, timeout-bounded waits, sleep-only exemptions, and semantic baseline comparison. Checker source changes activate both structural lint and its Python unit tests.
