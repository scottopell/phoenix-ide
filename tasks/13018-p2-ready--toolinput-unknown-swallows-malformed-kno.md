ToolInput::from_name_and_value silently degrades a malformed *known*-tool input into ToolInput::Unknown, so a real bug masquerades as "the LLM called a tool we don't have."

## Verified location
- crates/phoenix-ide/src/state_machine/state.rs:316-393 — `from_name_and_value` (fn starts at :316). Every known-tool arm (`bash`:318, `think`:325, `patch`:332, `keyword_search`:339, `read_image`:346, `spawn_agents`:353, `submit_result`:360, `submit_error`:367, `propose_task`:374, `ask_user_question`:381) is `serde_json::from_value(value.clone()).map_or_else(|_| ToolInput::Unknown { name, input: value }, ToolInput::Variant)`. A deserialization *failure* for a known tool produces the exact same `ToolInput::Unknown { name, input }` as a genuinely-unknown tool name (the `_ =>` arm at :388-391).

## Why egregious (correct-by-construction)
The type makes two semantically different situations structurally indistinguishable:
1. The LLM called a tool we do not support (legitimate Unknown).
2. The LLM called a tool we *do* support but with a payload our typed schema rejects (a bug — schema drift, a prompt regression, or an LLM error we want to surface/retry).

In case 2 two things are lost: (a) the serde error itself is discarded (no `tracing` at debug+ on the `Err` branch — a silent-capability-gap), and (b) the *typed interception* in `transition.rs` is bypassed. `transition.rs` matches on the typed variant (`matches!(t.input, ToolInput::ProposeTask(_))` / `ToolInput::AskUserQuestion(_)`), not on the name, so a malformed propose_task/ask_user_question never reaches the AwaitingTaskApproval / AwaitingUserResponse flow.

It is NOT necessarily "invisible" or "treated as an unregistered tool": `ToolInput::name()` (state.rs:289, `Unknown { name, .. } => name`) returns the original stored name, so the executor still dispatches by that name, and `propose_task`/`ask_user_question` have deliberate fallback `run()` paths that emit an explicit tool error (see `tools/ask_user_question.rs:91` — "This runs when the input failed to parse"). So the user-visible effect varies by tool: tools with a fallback `run()` surface a generic error and lose the typed flow + the precise serde diagnostic; the genuinely-unknown `_ =>` path is structurally identical. The defect is the lost typed interception and discarded serde error, not guaranteed invisibility. The existing test at state.rs:~418 only covers the genuinely-unknown case, not the malformed-known case — so the dangerous path is untested.

## Correct sibling pattern
The codebase routinely distinguishes "unsupported" from "malformed" with typed errors and logs the gap (e.g. `llm/anthropic.rs` normalize logs unknown blocks at `error`; `tools/mcp.rs:298-303` logs dropped block types at `debug`). A `ToolInput::Malformed { name, input, error }` variant (or returning `Result`) would make case 2 unrepresentable as a plain `Unknown`.

## Fix direction
Add a distinct variant for "known tool, failed to deserialize" carrying the serde error, OR have `from_name_and_value` return `Result` so the caller decides (surface a tool_result error + re-request the LLM, mirroring the existing `propose_task` validation-failure path at transition.rs ~1687-1708). At minimum, emit `tracing::warn!(tool = name, %err, "known tool input failed to deserialize; treating as Unknown")` on the `Err` branch and add a regression test for the malformed-known case.

## Related tasks
- 02665 (p3) — invalid (mode,state) co-constraints; adjacent defense-in-depth, not the same issue.
- 13016 (p2) — sibling correct-by-construction finding from the same audit.
