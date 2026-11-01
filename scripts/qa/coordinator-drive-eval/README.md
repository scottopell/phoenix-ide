# Isolated Coordinator drive evaluation

Opt-in action evaluation, not a general evaluation platform. See [RESULTS.md](RESULTS.md) for the 18 initial plus 8 reserved follow-up trials, limitations, returned usage, and the user's rejection of both representative prose samples.

The ignored Rust integration test uses the real `ModelRegistry`/`LlmService` provider path but **never imports application tools**. Four deterministic fake tools inspect fixed synthetic rows/history, record modeled delivered/queued-as-steering owner messages, or suspend for a structured question. No SQL is executed; no real worker, process, mutation API, or database is reachable. Expected outcomes are kept separate from model-visible fixture data.

## Credential-free preparation

From the repository root:

```sh
PHOENIX_COORD_EVAL_MODE=prepare \
PHOENIX_COORD_EVAL_MODEL=gpt-6-astra \
PHOENIX_COORD_EVAL_EFFORT=high \
PHOENIX_COORD_EVAL_REPETITIONS=2 \
PHOENIX_COORD_EVAL_VARIANTS=B,C2 \
PHOENIX_COORD_EVAL_SCENARIOS=quiet_subscription,executive_checkin \
PHOENIX_COORD_EVAL_OUTPUT=/tmp/coord-eval-prepared \
./dev.py qa coordinator-drive-eval
```

This writes a manifest and eight prepared snapshots without provider configuration or calls. Both modes require an output directory that does not already exist (with an existing parent); an existing directory, even empty, is rejected before evidence is written. The `live` mode uses the same command with a fresh output directory and explicit `MODE=live`; **it bills provider calls**. No further live calls are planned for this task after the recorded 26 trials. `dev.py` resolves the normal Phoenix environment without printing credentials. Missing model, unsupported effort, invalid selection, excess repetitions, or more than 18 trials in one invocation fail explicitly.

Each trial currently allows ten provider calls, 120 seconds per call, and 3072 output tokens per call. The original fixture/source at `0d7e78e46` allowed five calls and contained only A/B/C; its exact snapshots/manifests and limitations are preserved in `results/initial/`. Follow-up snapshots are in `results/followup/`. The manifest captures the exact fixture, and trial records capture system/messages/tools/results/usage/termination. Failed or timed-out calls withhold provider error payloads; returned error categories and termination remain visible. A successful ignored test means recording finished, not that model behavior passed.

## Variants

- A: frozen checkout-native coordinator prompt and exact evaluated read-only button.
- B: the same frozen prompt with the evaluated bounded active-coordination button in the four check-in cases.
- C: B plus initial appended principles.
- C2: B plus revised consolidated principles, replacing the conflicting unconditional receipt-reporting rule.

The subscription and continuation cases do **not** use the button; their A/B inputs are identical. The baseline is checkout-derived, not verified production-equivalent. `baseline-system-prompt.txt` is copied from the initial A trial and remains the source for historical variant construction after production prompt edits; the fixture buttons also remain unchanged. The selected product wording is not a fifth evaluated variant. Current fixtures are the revised follow-up fixtures; use recorded snapshots or the original commit for the original experiment. Do not silently compare changed fixtures, budgets, or baselines.

## Deterministic regressions

```sh
cargo test -p phoenix-llm --test coordinator_drive_eval
cargo clippy -p phoenix-llm --test coordinator_drive_eval -- -D warnings
python3 scripts/qa/coordinator-drive-eval/test_fixtures.py
```

These cover fixture/rubric separation, archived baseline parity, bounded calls, fresh output-directory rejection without overwriting evidence, modeled idle delivery versus working steering outcomes, recorded raw responses/results, unknown tools/targets, no-effects AUQ batch rejection, question suspension, exact message anchors, end-of-history, and the archived revised receipt rule. They do not qualify the selected product wording or replace human review.

## Simulation limits

SQL returns case rows independent of the query; SQL validity and schema discovery are not tested. Reads expose a small fixed history and simplified numeric paging. New fake sends return a simplified production-shaped `outcome`: `delivered` for idle fixture rows, `queued_as_steering` for working rows; invalid targets, IDs and unsupported states fail explicitly. This is a fixture-state approximation, not real admission or delivery proof. Sends never update owner state or fabricate completion; duplicate nudges are visible in raw actions, not automatically prevented. There are no watch-registration, shell, merge, deployment, lifecycle, or other production capabilities to exercise. A requested action is not recipient execution. The continuation case provides a fixed handoff and primary evidence; it does not test summary generation, full compaction, or C2 continuation behavior.
