OpenAI message translation drops nine history-bearing Anthropic server block variants behind a single content-free `tracing::debug!` — the log satisfies the letter of "capability gaps are logged" but is observability theater (no block type, no id, no count), and dropping history-bearing blocks can cause a later Anthropic-side 400 if a conversation switches providers mid-thread.

## Verified location
- crates/phoenix-ide/src/llm/openai.rs:580-592 — match arm collapsing `ContentBlock::ServerToolUse { .. } | ContentBlock::ToolSearchToolResult { .. } | ... | ContentBlock::McpToolResult { .. }` (nine variants, all `..`) into `tracing::debug!("Skipping Anthropic server block in OpenAI message translation");` — the log captures none of: which variant, tool_use_id, or how many were dropped.

(Reading-agent reported line range 580-592; re-confirm exact lines before editing — grep `Skipping Anthropic server block` in llm/openai.rs.)

## Why egregious
"Capability gaps are logged, not silenced" requires the gap to be *diagnosable*, not merely present in logs. Collapsing nine distinct variants with `..` and emitting a static string means the log can never tell an operator which block or how many were dropped — indistinguishable from a no-op when debugging a provider-switch 400. ServerToolUse / ToolSearchToolResult carry conversation-history state; silently omitting them from the OpenAI-translated history is data loss whose downstream symptom (Anthropic 400 on a later turn) is far from the cause.

## Correct sibling pattern
crates/phoenix-ide/src/llm/openai.rs:~762-764 — the adjacent `other` arm logs `tracing::debug!(output_type = %other, ...)`, capturing the discriminant. Also llm/anthropic.rs hard-errors (`LlmError::invalid_response`) on structurally-impossible blocks rather than ignoring them — a typed sink done right.

## Fix direction
Two separable concerns; decide explicitly:
1. Observability (low-risk): split the combined arm or bind the variant so the log records the block type and a count/id, matching the sibling at openai.rs:~762. This makes the gap diagnosable.
2. Correctness (needs a decision): determine whether ServerToolUse/ToolSearchToolResult/McpToolResult SHOULD be droppable at all when translating history for OpenAI, or whether they need a typed provider-capability representation so the omission is structurally a sink rather than an implicit `..`. This is the ambiguous part — owner must decide; do not silently keep dropping.

## Related tasks
- 13017 (p2 ready) — OpenAI cache-token silent drop; same provider, same "OpenAI path loses data the Anthropic path keeps" family, but a different field. Not a duplicate.
- 13015 (p2 ready) — NotifyClient stringly-typed/`_ => {}` silent arm; same silencing principle, different site.
