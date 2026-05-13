# Remind Explore agents to call `propose_task` after drafting task files

## Problem

In Explore mode, agents can use the scoped `patch` tool to draft a task file under `tasks/`, but they sometimes stop after the successful patch result and respond as if the task proposal is complete. The actual workflow requires a follow-up `propose_task` tool call so the user can review/approve the task and Phoenix can transition to Work mode.

The current system prompt already describes the two-step workflow, but the reminder is easy to miss after the model receives a terse patch success result:

```xml
<patches_applied>all</patches_applied>
```

## Recommended approach

Prefer a contextual patch-tool output reminder over a broader system-prompt change, but make the context correct-by-construction:

1. Replace the patch tool's anonymous optional allowlist field with a typed scope/policy enum that separates security behavior from workflow semantics. For example:

   ```rust
   enum PatchScope {
       Unrestricted,
       TaskProposalDraft { tasks_dir_name: String },
   }
   ```

   `TaskProposalDraft` means both:
   - the patch may only edit paths under the task directory; and
   - a successful patch is part of the Explore → `propose_task` workflow and should include a proposal next-step reminder.

   This avoids encoding workflow behavior as "`allowed_path_prefix.is_some()` happens to be true".

2. Expose constructors that make invalid states hard to represent.
   - `PatchTool::default()` / `PatchTool::unrestricted()` creates `PatchScope::Unrestricted`.
   - `PatchTool::for_task_proposal_drafts(tasks_dir_name)` creates `PatchScope::TaskProposalDraft { ... }`.
   - Avoid a generic public `restricted_to(...)` constructor unless there is an actual non-task use case; if tests need it, they should exercise the semantic constructor.

3. On successful `TaskProposalDraft` patches, append an LLM-visible next-step reminder that names the exact patched path, e.g.:

   ```xml
   <next_step>Call propose_task with task_file="tasks/92006-p2-ready--remind-propose-task-after-patch.md" if this is the task you want the user to approve.</next_step>
   ```

4. Keep unrestricted/default patch behavior unchanged.
   - No reminder for Direct/Work/Branch patch tools.
   - No reminder for Work-mode sub-agent patch tools.
5. Do not emit the reminder on failed patches.
   - Existing patch errors should remain unchanged, other than any internal refactor needed to use the typed scope.
6. Add tests covering:
   - `TaskProposalDraft` patch success includes a `propose_task` reminder with the actual patched task path.
   - `Unrestricted` patch success does not include the reminder.
   - `TaskProposalDraft` path enforcement still rejects paths outside the task directory.
   - `TaskProposalDraft` failures do not include the success reminder.
7. Avoid a synthetic system message unless the output hint proves ineffective; synthetic messages would require more runtime/state-machine plumbing and risk surprising history/UI behavior.
8. Avoid strengthening the global system prompt as the first fix; it increases prompt bulk for every Explore turn while the issue is specifically the post-patch moment.

## Base branch note

Before making code changes in Work mode, fetch latest `origin/main` and rebase/update the task branch onto it so the implementation starts from current main.

- `crates/phoenix-ide/src/tools/patch.rs`
  - Introduce the typed patch scope/policy enum.
  - Move allowlist enforcement behind the `TaskProposalDraft` variant.
  - Append the exact-path `propose_task` next-step reminder only for successful `TaskProposalDraft` patches.
  - Add/update unit tests near the existing restricted-patch tests.
- `crates/phoenix-ide/src/tools.rs`
  - Register `PatchTool::for_task_proposal_drafts(tasks_dir_name)` for Explore parent registries.
  - Ensure sub-agent and non-Explore registries use the unrestricted/default patch behavior.

## Acceptance criteria

- Explore parent registries use a semantically named task-proposal patch scope, not a generic path-prefix flag.
- After an Explore-mode task-proposal patch succeeds, the LLM-visible tool result explicitly reminds the agent to call `propose_task` with the patched task-file path.
- Non-Explore patch output is unchanged.
- Patch errors are unchanged except for any internal wording already tied to path enforcement.
- Tests document the scoped behavior and prevent regressing back to a generic `Option<String>` semantic check.
- `./dev.py check` passes.
