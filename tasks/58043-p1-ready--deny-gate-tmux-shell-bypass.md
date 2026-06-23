# Layer-0 deny rules don't cover tmux shell execution

## Problem

The Layer-0 permission gate `DenyGate::check` (`crates/phoenix-ide/src/runtime/deny_gate.rs`)
applies the shell-safety rules (`bash_deny_rule` -> `phoenix_tools::bash_check`)
**only when the tool name is `bash`**:

```rust
pub fn check(name: String, input: Value) -> Result<CheckedToolCall, Denial> {
    if name == "bash" {
        bash_deny_rule(&input)?;
    }
    Ok(CheckedToolCall { name, input })
}
```

`tmux_run` also accepts a shell `cmd`, and the pass-through `tmux` tool can
`send-keys` arbitrary text into a pane — neither is screened. An agent can route
a force-push, blind `git add`, or dangerous `rm -rf` around the deny rules by
running it through tmux instead of bash. So the "correct-by-construction, can't be
routed around" guarantee (REQ-PERM-001) holds only for `bash`, not for shell
execution generally.

Surfaced during PR #350 (user-guide) review by Codex; confirmed against code.

## Proposed fix

- In `DenyGate::check`, also vet `tmux_run`'s `cmd` with `bash_check` (the same
  AST checker) so force-push / dangerous-rm / blind `git add` are denied there too.
- Decide how to handle the pure `tmux` pass-through (`send-keys` is an
  unstructured payload, harder to screen statically): screen best-effort,
  constrain allowed subcommands, or knowingly accept out-of-scope and document.
- Update `specs/permissions/` so the requirement covers all shell-executing
  tools, not just `bash`.

## Acceptance criteria

- [ ] A force-push / dangerous-rm via `tmux_run` is denied identically to the
      same command via `bash`.
- [ ] Tests cover the `tmux_run` path (mirror the existing bash deny tests).
- [ ] The `tmux` `send-keys` surface has an explicit, documented decision
      (covered, constrained, or knowingly out-of-scope).
- [ ] `specs/permissions/` reflects the broadened scope.

## Notes

`docs/guide/concepts/permissions.md` currently scopes the guarantee to `bash` and
notes the `tmux_run` bypass (PR #350). Re-broaden that wording once the gate
covers tmux.
