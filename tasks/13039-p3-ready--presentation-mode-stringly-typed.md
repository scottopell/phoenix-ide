`ConvState::presentation_mode()` returns `&'static str` from a closed
set of 5 values. The UI consumes the value via `ts_rs`-emitted types
but loses exhaustiveness checking on the TS side because the return
type is `string`, not a union. A `ts_rs`-exported enum would let the
UI exhaustively match and would surface "added a 6th presentation
mode" as a TS compile error.

## Verified location

`crates/phoenix-ide/src/state_machine/state.rs:1610-1636`

```rust
/// Typed presentation mode for the frontend.
///
/// Maps states to the 5 presentation variants the UI renders.
/// Note: `ContextExhausted` always returns `"needs_action"` here.
/// Callers that have a full `Conversation` and want the `"done"` variant
/// for the case where `continued_in_conv_id.is_some()` must override this.
pub fn presentation_mode(&self) -> &'static str {
    match self {
        ConvState::Idle => "idle",
        ConvState::Error { .. } => "error",
        ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::ContextExhausted { .. } => "needs_action",
        ConvState::HandedOff { .. }
        | ConvState::Terminal
        | ConvState::Completed { .. }
        | ConvState::Failed { .. } => "done",
        ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. } => "working",
    }
}
```

## Why this matters (correct-by-construction)

AGENTS.md: "If a type permits a value that is semantically wrong, the
type is wrong -- fix the type, not the discipline."

`&'static str` permits any string. Today the function returns one of
five values; tomorrow someone adds a typo'd sixth ("workign") and
nothing on the TS or Rust side flags it. The doc comment claims "5
presentation variants" but the type does not enforce that count.

The docstring also calls out a callsite-level override: callers with a
full `Conversation` must coerce `"needs_action"` -> `"done"` when
`continued_in_conv_id.is_some()`. With an enum return, that override
becomes a typed `From`/`map` step instead of string surgery.

## Sibling pattern that does it right

The codebase already exports state-related enums to TS via `ts_rs`
(see `MessageType`, `ConvState` variant tags, etc.). The discipline
exists; this site was left as `&'static str` for legacy reasons.

## Fix direction

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
#[serde(rename_all = "snake_case")]
pub enum PresentationMode {
    Idle,
    Error,
    NeedsAction,
    Done,
    Working,
}
```

Then change `presentation_mode(&self) -> PresentationMode` and update
the consumers (UI: `parseConversationState` and downstream renderers).

Audit and update wire codegen -- `./dev.py check` will catch
generated/ drift.

## Related
- 02677 (rust-to-ts-sse-codegen, done -- the codegen substrate this
  would use)
