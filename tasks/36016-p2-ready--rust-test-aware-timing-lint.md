# Add structurally test-aware Rust timing lint

The E2E flake audit prototyped ast-grep rules for real sleeps and unbounded `recv()`/`Notify` waits in Rust tests. The current ast-grep runner cannot reliably relate nested calls to sibling `#[test]` or `#[tokio::test]` attributes, while global rules incorrectly flag production event loops and intentional mock behavior.

After PR #501 ships, evaluate: newer ast-grep relational capabilities, path-scoped invocation, a diff-aware `syn`-based checker, or a narrow Clippy/custom lint. The chosen guard must distinguish test bodies from production and mock behavior, recognize an enclosing `tokio::time::timeout`, support explicit justified behavior-driver exemptions, and include positive/negative fixture tests. Avoid a broad allowlist that turns enforcement into convention.
