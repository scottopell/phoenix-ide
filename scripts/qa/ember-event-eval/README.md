# Bounded completion-event comparison

This opt-in evaluation compares A: the production raw event packet, B: that
packet plus production occurrence-bound evidence, and C: the production hydrated
compact projection. It uses the real RuntimeManager tool registry and resolved
SpawnCatalog. Only real local query/read/search/reference/think tools execute,
against an isolated synthetic database. All other tools remain advertised but are
denied and recorded before dispatch. There are no fake SQL/history responses,
worker messages, shell commands, lifecycle operations or production DB access.

## Frozen decision rule

Before any trial, ADR-261 sets these promotion gates for B/C versus A:

- Zero attribution or authority failures in every required case.
- At least 25% fewer targeted source-message retrieval calls before a correct
  decision. A failed/incomplete answer does not count as a read saved.
- Each control case, including the eight-event batch, must keep accumulated
  provider-reported input-token regression within 10% and median total trial
  latency regression within 20%. Report controls separately so pooled cheap
  cases cannot hide the event-heavy result.
- Independently judge answers against `expected.json`, which is never sent to
  the model. The recording test passing means evidence was saved, not that the
  product passed these gates. Root integration review owns that judgment.

All variants share model, explicit effort, service tier, complete registry,
static system prompt, scenario/question and limits. Order rotates across cases.
The source fixture, prompt, tools and endpoint configuration are hashed. Returned
provider usage, cache/replay, all round-trip context bytes, read results, denied
actions and latency are retained. Bytes/words are not token or billing estimates.
Provider caches and service load are not controlled; report those limitations
and do not infer actual savings from packet length alone.

The event-heavy control captures eight distinct real direct-turn completions
from eight synthetic ordinary conversations, each ending in a tool-only response.
It persists eight independently identified event inputs to one Coordinator
transcript and hydrates them together into one initial context/wakeup. No event
ID, input ID, response ID or source identity is cloned to fake this workload.
All variants receive the same batch and question; its independent oracle rejects
invented broader completion, new authority and redundant worker nudges.

Limits: four targeted cases × three variants × one repetition, plus three control
cases × three variants × three repetitions: 39 trials total. Variant order
rotates between cases and control repetitions. At most twelve provider calls per trial,
1,536 output tokens and 60 seconds per call. The per-trial maxima sum to 468 calls,
but the hard global maximum remains 120, with a stop threshold of two million observed
input tokens for one invocation. The last returned call may cross that observed
threshold; it is checked before another call. No automatic retries. Cancellation/error/time
limits leave partial evidence and cannot qualify promotion. These are invocation
limits, not a claimed dollar ceiling or exactly-once billing guarantee. The twelve-call
per-trial cap applies uniformly to every variant and allows eight separate ending
reads plus a final answer and bounded catalog discovery. It does not relax the
global cap. The first six-call invocation was partial and unaccepted because all
three raw-event burst trials reached that limit without a final answer; its
[complete result report](results-e5ff1b8.md) remains part of the evaluation record.
The second invocation repeats all 39 trials with the same corpus, independent
oracles, ordering, model, effort, tools, output/time limits and promotion gates.
It is one explicitly authorized design iteration, with no selected-answer retry.

## Accepted component result

The complete second invocation passed the frozen gates and independent review
accepted C for component promotion: 39 final answers, 88 provider calls, zero
attribution/authority failures and 60% fewer targeted source reads. Each control
met the input/latency bounds. The [full result](results-cfda0b0.md) retains every
per-case measurement, hashes and limitations; the first invocation remains
[partial and unaccepted](results-e5ff1b8.md). Exact integrated CI and merge remain
separate gates.

## Run

```bash
PHOENIX_EVENT_EVAL_MODE=prepare \
PHOENIX_EVENT_EVAL_MODEL=gpt-6-astra \
PHOENIX_EVENT_EVAL_EFFORT=high \
PHOENIX_EVENT_EVAL_OUTPUT=/absolute/new/output-directory \
./dev.py qa ember-event-eval
```

`prepare` performs no provider call. `live` uses the same command with a new
output directory and `MODE=live`; it bills provider calls through the normal
Phoenix configuration without printing credentials. Existing output directories
are rejected. Model, connection and endpoint fingerprint remain in the manifest.
No raw production data belongs in fixtures or public results.

The fixture seeds durable input association in isolated SQLite and commits the
real terminal obligation/event path. It exercises production event capture,
prompt hydration and local retrieval; it does not claim full foreground admission,
compaction, remote-v1 enrichment, production deployment or human UI acceptance.

To reuse the exact compiled test harness while another lane builds, set
`PHOENIX_EVENT_EVAL_TEST_BINARY` to its absolute executable path. The wrapper
verifies the named test is present, records the binary SHA-256 and executes it
without invoking Cargo. No compilation or provider retry is hidden in this mode.
Authenticated skill reads are unexercised in this harness; their denials are
labelled `read_only_not_exercised`, not automatically classified as authority
failures. The independent rubric still judges the actual response and actions.
