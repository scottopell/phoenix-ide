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
- Select and render that message by reference from the frozen projection, then exclude only its ID when rendering trimmable history. Budget its full content plus request overhead first; fill the remainder with the existing suffix planner. Do not deep-copy the hydrated transcript.
- An unclassifiable provenance read closes the existing local-authority admission fence and propagates through the fatal exit path without recording a recoverable conversation failure. Established absence and known unusable input remain distinct.
- Missing/historical/reset acceptance input is reported explicitly without guessing or rereading removed text.
- Existing operation identity, retry, projection invalidation, and atomic summary commit remain shared. No new persisted policy or snapshot.

## Validation

- Deterministic tests cover both conversation variants, edited accepted input, ID-based deduplication, initial/historical/missing/reset cases, joint token/item budgets, recent corrections, and oversize failure before provider dispatch.
- Property coverage checks that the accepted handoff appears once and remains whole inside joint limits.
- Compare current instructions/retention, Coordinator instructions alone, and Coordinator instructions with protection across three successive compactions. Assess unresolved commitments, ownership, corrections, and resumption behavior, not just summary headings. Keep any live-model exercise bounded and separate from CI.
- Update bedrock and global-recall contracts and executive verification, plus ADR-049. Run focused tests, Allium/spec checks, and dev.py check.

## Repeated-compaction evidence

The opt-in `live_three_compaction_comparison` uses native provider requests with `gpt-5.5`, default effort, standard service tier, a 20,000-token simulated input window, and a 4,096-token output allowance. Each arm runs three compactions with enough completed fixture history to force trimming, followed by a no-tools resumption probe after each summary. The fixture mixes a paused Crick audit, Phoenix ownership succession, a canceled workstream, an explicit deployment ban, and unverified delegate reports.

| Arm | Observed behavior |
|---|---|
| Generic instructions and original trimming | Lost the paused Crick obligation and deployment ban at the first compaction, cancellation at the second, and exact Phoenix owner reference at the third. Later resumption starts repository inspection. |
| Coordinator instructions and original trimming | Better owner-first verification behavior, but the same earlier obligations and references disappear when the incoming handoff is trimmed away. |
| Coordinator instructions and protected handoff | All three handoffs retain the Crick owner, reproduction condition and promised follow-up, docs cancellation, deployment ban, and Phoenix owner succession. Delegate reports remain unverified and queued delivery is not promoted to acknowledgement or completion. |

This is one synthetic sample per arm, not a statistical benchmark, production-transcript replay, or guarantee of lossless summaries. The no-tools probes describe intended next actions; they do not verify tool execution. Input retention has deterministic regression coverage separately.

## Investigation and panel conclusions

Investigation began on local main 19fe992c7; production transcripts were not inspected. Roadmap #651 was read for coordination context.

The continuity reviewer emphasized dormant obligations, source/authority distinctions, and a forgetting policy. The runtime reviewer identified the accepted-message/original-summary distinction and the need to select from the same frozen projection. The simplicity reviewer recommended measuring instructions and retention separately and retaining existing output allowances.

The user settled retention simply: keep the accepted handoff in full, then fit recent history. The later scope expansion explicitly includes ordinary chats for retention. An ad-hoc project coordinator remains an ordinary chat; only the global Coordinator's instructions change.

Known limitation: older boundaries without accepted-message provenance cannot receive guessed protection. Existing global search excludes the Coordinator's own chain, so preserved direct transcript references matter for recovery.
