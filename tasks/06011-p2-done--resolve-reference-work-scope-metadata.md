# Expose authoritative WorkScope metadata in reference resolution

## Human commission

“file a task for this and queue it up as a parallel workstream, run it as a 5.6sol on medium effort”

One dedicated owner, gpt-5.6-sol with explicit medium effort, fresh isolated scope/branch. Owner may implement, test, publish and address review autonomously. Global alone merges and deploys. Do not interrupt existing mobile-list or provenance owners.

## Scope and acceptance

- Extend `resolve_reference` structured output with associated WorkScope ID, lifecycle, environment kind, authoritative cwd and worktree_path, and effective path choosing worktree_path then cwd.
- Stable conversation references select the current transcript's scope; exact transcript references select that exact historical member's scope. Never substitute a successor for exact-member lookup.
- Represent retired, missing and unavailable scope facts explicitly and honestly; do not invent defaults or paths.
- Mark server-side location semantics so a remote host's path is not implied local to the caller.
- Preserve access/security checks, Bash active-scope validation, existing execution authority and lifecycle. No DB mutations, lifecycle changes, migration by default, broad tool redesign or new execution capability.
- Focused tests: stable continuation lookup, historical exact lookup, retired/missing/unavailable scope, worktree-preferred effective path, cwd fallback, and different environment kinds.
- Coordinate the reference-tool seam with PR825 (qualified 8ce1959); add WorkScope metadata without duplicating its grammar implementation or legacy aliases. Coordinate source-call work PR827 without touching its ownership.
- Read normative reference/bedrock/workscope specs before edits; update relevant requirements/executive docs, codegen if needed, and keep schema/API output typed and lossless.
- Run focused tests and appropriate broader checks; publish a narrowly scoped PR, absorb concrete review findings, mark this task done and commit its rename when implementation is qualified. No self-merge or deployment.

## Initial isolation

Branch `feat/resolve-reference-work-scope`, based on main `2f2edf64f5a5a88c5fff704ec392fdd5d4f518a2`. Worktree is nested under the coordinator's allowed project target directory; do not edit any other worktree.
