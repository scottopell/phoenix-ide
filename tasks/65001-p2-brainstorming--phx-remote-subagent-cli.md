Remote Phoenix subagent + `phx` CLI (Rust mode of the existing binary).

GOAL
External processes (incl. Claude Code) fire a bounded task at Phoenix; it
runs autonomously with its real tool set + real LLM providers and returns
structured findings PLUS the full raw transcript. Unblocks driving the
Phoenix browser/perf tools from outside.

KEYSTONE — terminal result contract (build first)
- New tool `submit_result` (schema: summary + structured findings payload).
- New runtime/state-machine TERMINAL state `submitted`. A run is a
  "subagent" iff it calls submit_result; absence = incomplete/failed, by
  construction (correct-by-construction boundary, mirrors Claude Code
  subagents returning a final message).
- Spec it: specs/submit-result/ (executive.md min; Allium lifecycle since
  it adds a terminal state + precondition). Update relevant Allium specs.

CLI — `phx` as a MODE of the existing Rust binary
- `phoenix-ide phx ...`, reusing api/wire.rs types (NO client/server schema
  drift — same reason the UI uses ts-rs codegen). Zero extra deploy.
- Keep phoenix-client.py as the zero-install Python fallback.
- v1 verb: `phx run "<task>" [--cwd] [--model] [--max-turns N]
  [--timeout S] [--json]` -> create conversation, stream, block until
  submit_result (or terminal), emit {conversation_id, slug, result,
  transcript?}. `--raw` / `phx get <id> --raw` for the full structured
  transcript incl. raw tool-result blocks (paginated; it can be MBs —
  screenshots/DOM. Deliverable != raw dump; keep them separate).

DISCOVERY (formalize what dev.py/simple-client already do ad hoc)
- Server writes an instance registry on startup:
  ~/.phoenix-ide/instances/<id>.json = {pid, api_url, db, cwd, started}.
- `phx` API resolution precedence: --api > $PHOENIX_API_URL > nearest
  .phoenix-ide.dev.env (worktree) > registry match-by-cwd > prod default.

BOUNDS / RISKS (must be in v1)
- Autonomous + real providers = runaway cost: --max-turns/--timeout +
  cancel (/api/conversations/:id/cancel exists) enforced at the contract.
- Remote auth: localhost OK; network needs password/token path hardened —
  separate track, do not silently expose.

EXPLICITLY DEFERRED (falls out of the same capability later)
- Intra-chain / conversation->conversation history access. Once the
  "addressable structured transcript retrieval (chain-aware) + terminal
  contract" capability exists, an in-Phoenix `read_conversation`/
  `read_chain` tool is just the INTERNAL consumer of the same capability
  `phx` consumes externally. Design the capability with that second
  consumer in mind; do not build it in v1.

WHY: unblocks Claude Code (and any external orchestrator) driving Phoenix's
real browser/perf tool set autonomously; the perf-hunt suite currently
drives a browser via agent-browser as a stand-in.
