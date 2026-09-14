# Protect continuation handoffs and specialize global Coordinator compaction

## Approved scope

- All continuation-capable parent chats protect the full accepted previous handoff, including user edits, before allocating remaining input space to the newest messages.
- Only the global Coordinator gets coordination-focused summary instructions. Ordinary chats keep their coding-handoff instructions; no project-coordinator role detection or setting.
- Preserve unresolved workstreams, owners, promises, priorities, decisions, scoped authority, evidence, and next checks. Forget completed detail and repetitive history first.
- Use existing accepted-message provenance and the frozen current-member prompt projection. Do not guess a historical seed or resurrect reset text.
- If the full handoff and mandatory request overhead cannot fit, fail visibly through existing recoverable compaction failure rather than shorten/drop the handoff.
- No separate durable memory system, including as a fallback. No new capability, lifecycle, background monitoring, or whole-history replay.

## Done

A PR with relevant tests and repository checks passing, actionable review findings resolved, and clean Codex PR review on the final head. Merge and deployment are outside this request.

## Implementation

- Existing runtime Coordinator identity selects one of two instruction policies.
- A narrow storage lookup returns only the incoming accepted handoff's message ID.
- Select that message from the frozen projection by ID and remove it from the trimmable history. Budget its full content plus request overhead first; fill the remainder with the existing suffix planner.
- Missing/historical/reset acceptance input is reported explicitly without guessing or rereading removed text.
- Existing operation identity, retry, projection invalidation, and atomic summary commit remain shared. No new persisted policy or snapshot.

## Validation

- Deterministic tests cover both conversation variants, edited accepted input, ID-based deduplication, initial/historical/missing/reset cases, joint token/item budgets, recent corrections, and oversize failure before provider dispatch.
- Property coverage checks that the accepted handoff appears once and remains whole inside joint limits.
- Compare current instructions/retention, Coordinator instructions alone, and Coordinator instructions with protection across three successive compactions. Assess unresolved commitments, ownership, corrections, and resumption behavior, not just summary headings. Keep any live-model exercise bounded and separate from CI.
- Update bedrock and global-recall contracts and executive verification, plus ADR-049. Run focused tests, Allium/spec checks, and dev.py check.

## Investigation and panel conclusions

Investigation began on local main 19fe992c7; production transcripts were not inspected. Roadmap #651 was read for coordination context.

The continuity reviewer emphasized dormant obligations, source/authority distinctions, and a forgetting policy. The runtime reviewer identified the accepted-message/original-summary distinction and the need to select from the same frozen projection. The simplicity reviewer recommended measuring instructions and retention separately and retaining existing output allowances.

The user settled retention simply: keep the accepted handoff in full, then fit recent history. The later scope expansion explicitly includes ordinary chats for retention. An ad-hoc project coordinator remains an ordinary chat; only the global Coordinator's instructions change.

Known limitation: older boundaries without accepted-message provenance cannot receive guessed protection. Existing global search excludes the Coordinator's own chain, so preserved direct transcript references matter for recovery.
