# Durable Ember tracking

## User need

The user can return to commissioned work and useful questions after detours and continuations without reconstructing the request or maintaining a separate project manager.

### REQ-TRACK-001: Singleton memory

THE SYSTEM SHALL retain one list owned by the stable Coordinator aggregate, independent of its current transcript and generated summaries. An item SHALL distinguish requested work from a bookmark; neither stored prose nor a bookmark grants execution authority. Goals own worker execution and events supply facts; neither their completion nor PR completion SHALL automatically resolve a tracked outcome.

### REQ-TRACK-002: Durable structure and retirement

THE SYSTEM SHALL persist stable item identity, title, purpose/current note, historical resumption note, tracking/resolved/dismissed disposition, checked integer-microsecond timestamps and monotonic revisions. Resolved/dismissed items SHALL require a nonblank reason and remain available for inspection and subsequent matching. Ordered typed references/evidence SHALL be child rows, retained independently of source or peer deletion. The system SHALL NOT automatically expire items.

### REQ-TRACK-003: Native operations and authority

THE current Coordinator SHALL have bounded list/read/record/update/retire/reopen operations through one native tool and application service. Source transcript and executing call identity SHALL come from trusted context and be revalidated transactionally. Ordinary conversations, children and predecessor Coordinator transcripts SHALL NOT mutate the list. Mutations SHALL compare expected revisions; exact trusted-call retries SHALL return retained receipts, while changed intent under the same call identity SHALL conflict. Unclassified local SQLite authority SHALL close runtime authority before ordinary publication.

### REQ-TRACK-004: Evidence and reconciliation

New source observations SHALL append evidence, not mutate previously reviewed observations. A derived evidence-since-review projection SHALL distinguish historical notes needing reconciliation from an explicitly reconciled resumption point. New evidence SHALL invalidate the ready-next-step projection atomically; identical evidence SHALL not manufacture review work. Explicit reconciliation SHALL compare item and evidence revision, preserving user restrictions. Reviewed is not permission. Scanner append SHALL NOT rewrite notes, advance review, retire/reopen, grant authority or lift a pause.

### REQ-TRACK-005: Bounded inspectability

List/detail results SHALL identify captured registry/item revisions, counts, omissions and continuation pointers. Saved historical notes, new evidence and reviewed resumption points SHALL be visibly distinct. Missing sources SHALL remain explicit uncertainty. Full notes/evidence SHALL be read on demand, with bounded pages. An omitted item SHALL NOT be represented as reviewed, complete or ready.

### REQ-TRACK-006: Prefix-preserving continuity

At existing admitted Coordinator turn/wakeup boundaries THE SYSTEM SHALL append changed bounded tracking observations to durable transcript tails. It SHALL NOT change system instructions, prepend changing state, rewrite earlier messages or repeatedly append unchanged state during model/tool loops. Current registry data SHALL remain recoverable after continuation independently of its summary. Observations SHALL be labelled captured historical data, never user instructions or current worker status.

### REQ-TRACK-007: Useful, quiet attention

THE Coordinator SHALL use normal turns and existing wakeups to recover relevant older intent and occasionally revisit neglected requested outcomes. These paths SHALL share at most one useful cue, often none, and a wall-clock cooldown. A recently reviewed unchanged small list SHALL NOT alternate every turn. No item due SHALL cause no extra recital, worker nudge or wakeup. A bookmark SHALL remain a question/resumption aid rather than a commission. Saving and background scanning SHALL NOT schedule foreground work.

A cue SHALL apply only to the admitted input before its appended observation. A later input SHALL make that retained cue historical rather than renewing a review opportunity. Surfacing SHALL NOT advance explicit review.
