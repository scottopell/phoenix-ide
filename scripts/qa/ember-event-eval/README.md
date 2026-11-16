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
- Control-case accumulated provider-reported input tokens regress at most 10%;
  median total trial latency regresses at most 20%.
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

Limits: four targeted cases × three variants × one repetition, plus two control
cases × three variants × three repetitions: 30 trials total. Variant order
rotates between cases and control repetitions. At most four provider calls per trial,
1,536 output tokens and 60 seconds per call, 120 calls and a stop threshold of two million observed
input tokens for one invocation. The last returned call may cross that observed
threshold; it is checked before another call. No automatic retries. Cancellation/error/time
limits leave partial evidence and cannot qualify promotion. These are invocation
limits, not a claimed dollar ceiling or exactly-once billing guarantee.

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
