# Eliminate Git signing prompts from tests and structurally guard Git subprocesses

## Problem

Rust test execution can invoke the user's configured Git signing program. `./dev.py check` first attempts a signed probe commit before deciding whether to disable signing, and `phoenix-tools::commission_review` creates test commits through a raw `Command::new("git")` helper. With SSH signing backed by 1Password, ordinary test runs can display or block on a system authorization prompt.

Phoenix already has the right subprocess convention in `phoenix-ide/src/git_ops.rs`: inject `commit.gpgsign=false` through Git's process-level `GIT_CONFIG_COUNT` mechanism. That convention is not available to lower-level crates, however: `phoenix-tools` cannot depend on `phoenix-ide` because `phoenix-ide` already depends on `phoenix-tools`.

## Plan

1. Remove the signing-capability probe from `./dev.py check`. Whenever cargo lanes run, unconditionally add a process-only `commit.gpgsign=false` override to the child environment. Preserve and compose with any pre-existing `GIT_CONFIG_COUNT` entries rather than overwriting them.
2. Extract the common Git command/config construction to a dependency-safe home in `phoenix-core` (or an equivalently lower shared crate). Keep `phoenix-ide::git_ops` as the domain-facing API and make it delegate to the shared constructor, so there is one authoritative implementation of signing suppression and standard Git subprocess configuration.
3. Route `phoenix-tools::commission_review` Git execution, including its test commits, through that shared constructor while preserving its async process, cancellation, `GIT_OPTIONAL_LOCKS`, and `GIT_NO_LAZY_FETCH` behavior.
4. Audit literal Rust Git subprocess spawns across `crates/`. Migrate runtime and test call sites that can use the shared constructor. Retain narrowly justified boundary exceptions only where normal crate dependencies are unavailable (notably a Cargo build script) or where a test deliberately exercises raw/sandboxed Git behavior; each exception must still explicitly disable signing if it can create a commit.
5. Add an ast-grep rule that rejects literal Git process construction (`Command::new("git")`, including qualified std/tokio forms) outside the authoritative shared Git module and explicit boundary exceptions. The useful definition of “all” is: all statically visible Rust subprocess creation whose executable is the literal `git`. The guard intentionally does not claim to detect dynamically computed executable names or Git invoked inside arbitrary shell text.
6. Add regression coverage using hostile Git configuration/signing-program behavior to prove that both direct `cargo test` and the `dev.py` test environment disable commit signing without first invoking the signer. Add structural-rule fixtures/tests if the existing ast-grep infrastructure supports them.
7. Update the agent-identity specification/current-status documentation as needed, keeping timeless requirements separate from implementation status.
8. Run focused tests, the ast-grep lane, `./dev.py` unit tests, and `./dev.py check`.

## Acceptance criteria

- `./dev.py check` never probes or invokes the configured signing program and unconditionally supplies `commit.gpgsign=false` to cargo/test children.
- Direct `cargo test` for `phoenix-tools` commission-review tests cannot invoke the user's signer.
- Signing suppression and common Git subprocess conventions have one dependency-safe implementation used by `phoenix-ide::git_ops` and `phoenix-tools`.
- CI rejects new literal `git` process spawns outside the approved shared boundary/explicit exceptions.
- Existing Git behavior, async cancellation, bounded output, sandbox tests, and build metadata continue to work.
