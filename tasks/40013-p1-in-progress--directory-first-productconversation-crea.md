# Directory-first ProductConversation creation

Child of parent task 92009. Replace the normal web creation journey with one typed directory-first ProductConversation protocol: server-derived Git/non-Git behavior, reserved canonical root identity, detached WorkScope worktree ownership, hidden GitRepository attachment, recent management-root suggestions, canonical navigation, and truthful durable recovery. Legacy Project/mode/branch inputs remain unreachable compatibility only. Close/member lifecycle and task 40010 are out of scope.

## Persisted handoff

PR #727 head `aea457047e` has green exact-head CI and an exact-head Codex review with eight new valid findings. The worktree contains uncommitted partial remediation in `migrations.rs`, `api/types.rs`, and `api/lifecycle_handlers.rs`: migration 075 reservation table stub, reservation ID/unresolved wire variants, and removal of the obsolete Project approval gate. These edits are intentionally incomplete and unvalidated.

Next coherent slice:
1. Finish normalized `product_root_reservations` persistence and typed reserve/consume validation, using integer Unix-microsecond timestamps and durable unresolved reservations.
2. Thread scalar form-decodable reservation fields through file/skill queries and TS; implement mkdir → reserve → create for `will-create`.
3. Make worker consume the durable reservation only, persist unresolved failure after shell acceptance, and claim-fence hidden-repository attachment.
4. Admit approval from attached WorkScope/hidden repository rather than `project_id`, without changing resources/mode.
5. Prevent repository identity reuse from path alone and rank recent roots by distinct ordinary ProductConversation IDs.
6. Add focused tests, regenerate TS, run full check, immutable review, push, exact-head Codex/CI, resolve all threads, then mark task done.
