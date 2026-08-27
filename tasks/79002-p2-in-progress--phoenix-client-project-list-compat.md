# Accept both project-list response shapes in phoenix-client.py

## Observed journey

- Running `phoenix-client.py --list-projects` against Phoenix 0.12.0 reaches `PhoenixClient.get_projects()` and receives a bare JSON list from `GET /api/projects`.
- The client currently assumes the decoded response is an object and calls `.get("projects", [])`, so the bare-list response is incompatible.
- Work is confined to the current isolated worktree. Do not modify the main checkout, push, or open a PR.

## Verified findings

- `phoenix-client.py::PhoenixClient.get_projects` performs the request, raises for HTTP errors, then returns `resp.json().get('projects', [])`.
- `crates/phoenix-ide/src/api/handlers.rs::list_projects` serializes the database result directly, producing a bare JSON array.
- Focused Python client coverage lives in `tests/test_phoenix_client.py` and uses `unittest` plus mocks.

## Inferences and unknowns

- The compatibility boundary belongs in the standalone client: accepting the server's bare list fixes 0.12.0 while retaining support for the existing wrapped `{ "projects": [...] }` shape.
- No product decision or broader API migration is needed.

## Interaction map

- Phoenix server `GET /api/projects` → HTTP JSON response (bare list in 0.12.0, historically/client-expected wrapped object) → `PhoenixClient.get_projects()` → `--list-projects` output.
- No persistence, recovery, cancellation, SSE, or UI paths are involved.

## Proposed scope

1. Update `PhoenixClient.get_projects()` to decode the response once and return a bare list directly, while preserving the existing wrapped-object extraction and empty-list fallback behavior.
2. Add focused regression tests in `tests/test_phoenix_client.py` for:
   - a bare project list;
   - a wrapped `{ "projects": [...] }` response, proving current behavior remains supported.
3. Run the focused Python client test file and report the command/result.

## Acceptance evidence

- Both response-shape regression tests pass.
- Existing tests in `tests/test_phoenix_client.py` pass.
- The diff is narrowly limited to `phoenix-client.py` and its focused test file (plus this task lifecycle artifact if retained by the workflow).

## Non-goals

- Changing the server endpoint or wire contract.
- Refactoring unrelated client response parsing.
- Adding broad malformed-payload validation.
- Modifying specs, UI, Rust code, or deployment behavior.
- Pushing, opening a PR, or touching the main checkout.
