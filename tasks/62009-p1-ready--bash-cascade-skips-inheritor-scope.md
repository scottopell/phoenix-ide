---
created: 2026-05-24
priority: p1
status: ready
artifact: crates/phoenix-ide/src/api/handlers.rs
---

Bash cleanup cascade is conv-id-keyed and does not consult inheritor_scope.

VERIFIED LOCATION

- `crates/phoenix-ide/src/api/handlers.rs:2118`:
  ```rust
  cascade_bash_on_delete(state.runtime.bash_handles(), id)
  ```
- Compare `crates/phoenix-ide/src/api/handlers.rs:2143-2148` (tmux) and `:2174-2179` (browser):
  both take `(&work_scope, inheritor_scope.as_ref())` and skip kill when
  scopes match (REQ-TMUX-WS-002, REQ-BROWSER-WS-002/003).
- `cascade_bash_on_delete` signature: `crates/phoenix-ide/src/tools/bash/registry.rs:312`
  — `(registry, conv_id)`. No work-scope parameter at all.

WHY IT MATTERS

Continuation-chain UX is now structurally asymmetric:

| Resource | Parent archive while continuation alive |
|----------|------------------------------------------|
| tmux     | Preserved (scope inherited)              |
| browser  | Preserved (scope inherited)              |
| bash     | SIGKILLed                                |

LLM in continuation conv sees its tmux server + browser session intact
but its long-running `cargo build` (spawned in parent) was killed with
no notification. From the LLMs vantage point the runtime randomly
forgot half its in-flight work.

DESIGN QUESTION (resolve before fixing)

Two coherent positions:

A. Bash handles SHOULD inherit. Symmetric with tmux/browser. Requires
   either (a) re-keying registry by WorkScope or (b) transferring entries
   from parent conv-id to continuation conv-id on the inheritance edge.

B. Bash is intentionally per-conv-ephemeral; tmux is the documented
   answer for "needs to survive boundaries". Make the asymmetry
   *visible*: the bash cascade on archive-with-continuation emits a
   user/LLM-visible notification listing the killed handles, so the
   LLM in the continuation knows what was lost. Document this as
   contract in `specs/bash/`.

(A) is the deeper fix and unblocks the unified WorkHandle abstraction
that phase 2 wants. (B) is a one-PR clarification of existing intent.

RELATED

- Phase 2 spec (`specs/wake-contracts/`, in-flight) will need to resolve
  this: if a wake contract is registered on a handle that gets cascaded-
  killed by inheritance asymmetry, the contract fires `forgotten` with
  no useful payload.
- Persona-panel (Mira Goldberg, failure-mode investigator) flagged the
  silent-loss path; design framing here is more nuanced than her
  framing (bash is conv-keyed by design, not WorkScope-keyed).
