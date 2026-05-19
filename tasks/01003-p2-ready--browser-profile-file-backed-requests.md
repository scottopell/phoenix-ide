# `browser_profile` file-backed `run_scenario` requests

Add file-backed request/response support to the existing `browser_profile` tool so script-generated performance scenarios can be handed to the tool without manual JSON copying or a separate `phx tool` daemon bridge.

## Goal

The Phoenix perf skill already translates declarative YAML scenarios into canonical `browser_profile run_scenario` JSON requests:

```bash
skills/phoenix-perf-shared/scripts/run-scenario \
  skills/phoenix-perf-find-target/resources/scenarios/conversation-load.yaml \
  --request-out /tmp/pp-request.json
```

Today the parent agent must manually read that JSON, map it into the `browser_profile` tool call fields, capture the tool result, write it back to a response file, then run:

```bash
skills/phoenix-perf-shared/scripts/run-scenario \
  --response-in /tmp/pp-response.json \
  --out /tmp/raw.json
```

This task removes the manual middle step by teaching `browser_profile(action="run_scenario")` to accept a request JSON file and optionally write the complete response JSON to a file.

## Non-goals

- Do not build `phx tool` or any daemon CLI bridge in this task.
- Do not expose browser_profile to shell scripts without an agent tool call.
- Do not change browser session ownership; the tool remains conversation-scoped exactly as today.
- Do not make `browser_profile` parse the perf skill's YAML scenario format. The skill runner owns YAML → canonical request translation.
- Do not compute statistics in `browser_profile`; it must continue returning raw samples only.

## Tool shape

Extend the existing `browser_profile` tool schema for `action="run_scenario"` with optional fields:

```json
{
  "action": "run_scenario",
  "request_file": "/tmp/pp-request.json",
  "output_file": "/tmp/pp-response.json"
}
```

Semantics:

- `request_file` points to a JSON object containing the same fields currently accepted directly by `run_scenario` (`reset`, `steps`, `runs`, `warmup`, `throttle_rate`, `gc_per_run`, etc.).
- The tool reads and validates the request file, then executes exactly as if those fields were provided directly in the tool call.
- Direct inline fields remain supported for normal LLM-authored calls.
- If both `request_file` and inline request fields are provided, fail clearly rather than merging two sources of truth.
- `output_file`, when provided, receives the complete JSON response from `browser_profile`, including `raw_samples`, `methodology_warnings`, `requested_runs`, `warmup`, and any metadata.
- The tool response shown to the LLM may be a concise summary when `output_file` is used, but it must include the output path and enough metadata to confirm success. It must not silently drop errors.

## File safety

Use the same path safety posture as other Phoenix tools that read/write local files:

- Paths are interpreted in the agent/server filesystem, not the browser page.
- Missing `request_file` fails clearly.
- Invalid JSON fails clearly with filename and parse error.
- A syntactically valid JSON file that violates `run_scenario` schema fails before launching any measured runs.
- `output_file` parent directory must exist; fail clearly if it does not.
- Do not write partial success responses on failed runs unless the existing `browser_profile` failure contract explicitly includes samples. The current readiness-timeout behavior returns zero samples and should stay that way.

## Perf skill workflow after this task

Expected flow:

```bash
skills/phoenix-perf-shared/scripts/run-scenario \
  skills/phoenix-perf-find-target/resources/scenarios/conversation-load.yaml \
  --subst UI_URL=http://localhost:8045 \
  --request-out /tmp/pp-request.json
```

Agent tool call:

```json
{
  "action": "run_scenario",
  "request_file": "/tmp/pp-request.json",
  "output_file": "/tmp/pp-response.json"
}
```

Then:

```bash
skills/phoenix-perf-shared/scripts/run-scenario \
  --response-in /tmp/pp-response.json \
  --out /tmp/pp-raw.json
```

The harness still emits raw per-run samples only; `stats.py` remains the only reducer.

## Acceptance criteria

- `browser_profile(action="run_scenario", request_file=...)` executes the same request shape as inline `steps`/`runs`/`reset` calls.
- `output_file` writes the complete response JSON, including raw samples and methodology warnings.
- Inline request fields and `request_file` are mutually exclusive; mixed calls fail with a clear error.
- Invalid/missing request files fail before browser interaction begins.
- The perf suite's `--request-out` → `browser_profile request_file/output_file` → `--response-in` workflow works for at least `conversation-load`.
- Existing inline `browser_profile run_scenario` behavior and tests remain unchanged.
- Add tests for request-file parsing, mutual-exclusion validation, output-file writing, and schema validation failure.

## Deferred

A future `phx tool browser_profile ...` daemon bridge may still be useful for fully autonomous shell-only workflows. Defer that until file-backed `browser_profile` proves insufficient.
