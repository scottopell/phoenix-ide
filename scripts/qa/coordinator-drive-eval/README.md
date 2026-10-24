# Isolated coordinator drive evaluation

`fixtures.json` contains **synthetic** authentic primary-user history, live current rows, and exact messages returned by the fake read tool for six cases. `expected.json` is a separate, human-scored rubric and is never included in an LLM request. No real application tools, SQL, message delivery, or database are used. In cases 1/2/3/5, A sends the **exact** current `COORDINATOR_BRIEFING_PROMPT` and is intentionally read-only; B asks for bounded whole-stack action under *existing* standing authority; C uses the same B request plus compact system principles. Subscription and continuation use identical A/B input history without either button; C changes only the system principles. The Rust regression test fails if the UI button and frozen A differ.

The executable is the ignored Rust integration test `crates/phoenix-llm/tests/coordinator_drive_eval.rs`. Run deterministic tests (no credentials/provider calls):

```sh
cargo test -p phoenix-llm --test coordinator_drive_eval
python3 -m unittest scripts/qa/coordinator-drive-eval/test_fixtures.py
```

Prepare snapshots and manifest **without billed calls** (the model and effort are recorded but not loaded):

```sh
PHOENIX_COORD_EVAL_MODE=prepare PHOENIX_COORD_EVAL_MODEL=gpt-6-astra \
  PHOENIX_COORD_EVAL_EFFORT=medium PHOENIX_COORD_EVAL_REPETITIONS=1 \
  PHOENIX_COORD_EVAL_OUTPUT=/tmp/coord-eval-prep \
  cargo test -p phoenix-llm --test coordinator_drive_eval coordinator_drive_eval -- --ignored --exact
```

Only `MODE=live` enables billed calls, and only with explicit model, effort, and output directory. Use the same command with `PHOENIX_COORD_EVAL_MODE=live` after checking the model/effort and credentials in the usual dev environment (the parent wires the `dev.py` runner). Optional `PHOENIX_COORD_EVAL_VARIANTS=A,B,C`, `PHOENIX_COORD_EVAL_SCENARIOS=idle_owner,continuation_pause`, filter runs. `PHOENIX_COORD_EVAL_REPETITIONS=1|2` is required and capped at two. Without filters all three variants × six scenarios run. Hard bounds: five provider calls/trial, 120 seconds/call, 3072 output tokens/call. A question suspends the trial; it must be the sole tool call in a response, so independent sends must occur beforehand.

Each trial produces a JSON file with exact system and user prompts, model/effort, synthetic evidence, per-call request messages and raw public response blocks, usage, fake tool arguments/results, actions, and termination. Provider/private replay is retained in memory for successive calls but only its **kind/count** is recorded; raw credentials, headers, provider error strings, and private reasoning are not persisted. A fake accepted/queued send is **not** verified completion. Records support human scoring against `expected.json`; there is no claimed statistical significance or production-runtime replay.
