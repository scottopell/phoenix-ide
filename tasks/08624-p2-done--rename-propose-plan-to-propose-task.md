---
created: 2026-04-03
priority: p2
status: done
artifact: src/tools/propose_plan.rs
---

# Rename propose_plan to propose_task

## Summary

The tool is called `propose_plan` but it creates a task. The approval UI says
"task plan" in some places and "task" in others. The thing being proposed is a
task; the plan is the content of the task. Aligning the name eliminates the
plan/task confusion.

## What to change

- Rename tool: `propose_plan` -> `propose_task` in tool name, description, schema
- Rename file: `propose_plan.rs` -> `propose_task.rs`
- Update all references: state machine interception, system prompt mentions,
  tool registry, error messages, specs
- Update UI text: "task plan" -> "task" in TaskApprovalReader discard dialog
- Update `ToolInput::ProposePlan` variant -> `ToolInput::ProposeTask`

Keep the tool input field named `plan` (the body IS a plan). Only the tool
name and references change.

## Done when

- [ ] Tool registered as `propose_task`
- [ ] All code references updated (grep for propose_plan returns 0)
- [ ] UI approval text says "task" not "task plan"
- [ ] Existing conversations with propose_plan in history still work
  (strip_unavailable_tool_blocks handles the old name)
