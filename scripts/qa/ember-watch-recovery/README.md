# Isolated local-watch recovery qualification

One bounded case tests whether a configured real model recovers from sanitized
historical flat/local-`after` failures using the current assembled coordinator
surface. The history is explicitly labelled synthetic retained observations. It
is not fabricated execution output from the current tools.

The fixture creates one actual open ordinary conversation and enrolls its actual
local watch in an isolated database. All advertised definitions come from
`RuntimeManager::coordinator_tool_registry` and production spawn specialization,
including authenticated skills. It uses the normal coordinator prompt. Local
list/think/query/read/search/resolve calls execute the actual registered tool.
Remote selections and effectful tools are denied before dispatch. Unsupported
read-only helpers are recorded as `read_only_not_exercised`, not authority failures.
No production database, peer transport, shell or target action is exercised.

## Frozen budget and oracle

- One case, one repetition, exact `gpt-6-astra`, explicit high effort, no fallback.
- At most four provider requests and four dispatched/denied tool calls total.
- Each request: 60-second timeout and 1,536 output tokens.
- Stop before the next request when observed input including cache reaches 40,000
  tokens. This is a boundary stop threshold; the final request can cross it.
- To qualify, the model must submit `{"request":{"op":"local"}}` to the actual
  registered `list_watched_conversations` and receive the enrolled target. It must
  not guess `after` UUIDs, select a peer or attempt effectful actions. Review the
  final answer against the actual returned watch. Read-only helper denials must
  be reported separately and must not be mislabeled as authority violations.

A successful test process records evidence; it does not certify the oracle.
Prepared route, prompt/definition hashes and exact binary must be reviewed before
live execution. Provider, timeout or budget failure is an unqualified result;
never silently rerun or enlarge the budget. Preserve the original evidence.

## Running

Compile with the owning `--lib` test, then use an immutable copy of that executable
with the normal environment wrapper. The output parent must exist; each output
directory must be new.

```bash
PHOENIX_TOOL_RECOVERY_MODE=prepare \
PHOENIX_TOOL_RECOVERY_MODEL=gpt-6-astra \
PHOENIX_TOOL_RECOVERY_OUTPUT=/absolute/new/prepared-directory \
PHOENIX_TOOL_RECOVERY_TEST_BINARY=/absolute/qualified-test-executable \
  ./dev.py qa ember-watch-recovery
```

After reviewing `recovery.json`, use `MODE=live` and a different fresh directory.
Keep preparation and live artifacts separate. The wrapper validates the binary's
exact named test and records its digest; it does not choose another model.

This proves read-only model selection and actual registered dispatch for this
synthetic configuration. It does not exercise full runtime turn admission,
durable tool-round checkpointing, production activation or every provider. Rerun
the inventory after tracking/event integration to qualify the final composition.
