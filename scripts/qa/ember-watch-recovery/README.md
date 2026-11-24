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


## Qualification receipt

The integrator independently accepted the one live trial from runtime source
`2e098a3021600cb997d3ed68e457559d9f596977`. Preparation and execution used identical
route, binary, system prompt, definitions, limits and oracle. Actual discovery
selected `gpt-6-astra` / requested high effort / `codex`, PhoenixNative language
and the authenticated catalog. The emitted arguments were exactly
`{"request":{"op":"local"}}`; the registered tool returned the enrolled synthetic
**Watch Recovery Sentinel**, with its exact stable identity, transcript and idle
state. The final answer named that one conversation and its idle state correctly.

Two provider requests, one tool call, 21,921 observed input tokens including cache
and 74 output tokens were recorded. There were zero UUID guesses, remote
selections, effectful attempts, helper denials or errors. The test process took
6.95 seconds; that is whole-trial wall time, not a provider latency benchmark.
No budget change or second trial was used.

- Binary SHA-256: `af678fd42762b5444e5831d6597a78cc80deb68ea141ca5a5a77d3965910e60e`.
- System prompt SHA-256: `787109b6cc371b0b51a0a62105c918bfc5a529ab6e3c4af070c4774bc82966f0`.
- Tool definitions SHA-256: `62b3c1346f994c03cca1465ff2f0af1150fadc00da16dddb3c9ca262b60c70ae`.
- Prepared trace SHA-256: `daee18480acd73deb604060401bcc51db459bc409b1dbbfceddf0fb8b54d1a86`.
- Live trace SHA-256: `7b228ca4d2563d34862aa5cf38076e08035950449929ce1483cf761d5c4d009b`.

Private raw artifacts are retained under the audit worktree's
`.phoenix/qualification/tool-audit/recovery-2e098-{prepared,live}/recovery.json`.
Only this sanitized summary and hashes are committed. The isolated deterministic
fixture also passed actual list/query/read/search/resolve/think smoke calls before
provider execution. Full runtime admission, deployed behavior and the final
integrated tracking/event surface remain outside this receipt.
