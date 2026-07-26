Child of task 92010.

Concern:
- Establish the terminology and authority baseline across the affected requirements so the spec set uses one settled vocabulary and one source-of-truth story for conversation lifecycle ownership.

Settled terminology guardrails:
- Use “conversation lifecycle” for the user-visible lifecycle model.
- Use “Open”, “Close”, and “Closed” only where the requirements intentionally define lifecycle states or actions.
- Treat “authority” as the normative owner of lifecycle truth; avoid parallel terms such as “real owner”, “actual source”, or “canonical-ish”.
- Preserve already-landed durable direct-turn terminology where it is still normative; do not invent replacement terms unless the requirements explicitly settle them.
- Call legacy names “legacy” when describing mappings; do not let old schema labels masquerade as current requirements terminology.

Done evidence:
- Affected requirements files use one consistent lifecycle vocabulary.
- Requirements clearly identify which artifact is authoritative for lifecycle truth and which artifacts are derived or legacy-facing.
- Any terminology deltas needed by downstream Allium/ADR work are called out in the task notes or commit message for follow-on child tasks.
- Validation evidence cites the exact requirements files touched and confirms no status/progress language was introduced into timeless artifacts.
