# Conversation observation — current evidence

## Status

The reusable server/CLI/UI capability closure is committed as `d3c756e98` on
`bench/current-readonly-history-compat`, based on `713c74a7d`. A subsequent
source-only correction supplies loopback ConnectInfo to the normal login
regression request; its containing commit identifies that corrected candidate.
The stock service on port 18043 and its immutable fixture remain unchanged.

The first shared-projection unit `196fc1e3d` was actually qualified by hosted run
39039158387 (harness `10bf43bc8`): the exact contract test passed (1), the owning
product API suite passed (33), Clippy with warnings denied passed, and the real UI
embed build passed. These receipts do not qualify the later state/CLI/UI closure.
That closure has source review and local JavaScript/Python evidence only; hosted
Rust tests, actual codegen, binary admission and browser acceptance remain owed.
No local Rust build, fixture upload or candidate server launch occurred.

Roadmap issue #806 was read. Baseline observations in this matrix refer to the
locally audited `origin/main` at `d07417325`, not an assertion about subsequent
main or deployment. The broader observation feature is retained branch work,
not a landed supported release.

## Landed versus retained gap matrix

| Capability / requirement | Landed baseline evidence | Retained/worktree evidence | Gap / acceptance still owed |
|---|---|---|---|
| Read-only SQLite primitive, REQ-COBS-003 | Local `origin/main` contains `Database::open_read_only` in `crates/phoenix-db/src/lib.rs` | Startup calls this primitive after standalone admission | No binary qualification here; read-only primitive alone does not prove isolation or schema sufficiency |
| Shared typed persisted history projections, REQ-COBS-004 | Not established as landed by this bounded audit | `196fc1e3d`, hosted contract 1/API 33/Clippy passed | Later closure and deployment/UI parity remain separate gates |
| Immutable fixture and scoped observation server, REQ-COBS-003/004 | Local `origin/main` CLI has no observation invocation; `api/application_state.rs` absent | Committed retained `main.rs`, `immutable_fixture::standalone_path`, `observation_startup::run`, `ObservationApplication`, scoped `ConversationRouteObservations` and authored startup tests | Server owner must integrate/review and qualify exact binary: admission/denial, no migration/writes/live services, target scope, loopback/forwarding, startup/shutdown and supported schemas |
| No-build user command, REQ-COBS-001 | Absent from audited baseline | `dev.py observe`, explicit `--binary`, `--fixture`, `--target`, optional `--port` default 0 | Seven mocked Python tests pass; no actual executable launch claimed |
| Environment/process isolation, REQ-COBS-002 | No observation launcher established | Empty scratch cwd, private HOME/XDG/tmp, exact child-env allowlist, signals/status/cleanup tests | Real candidate process/filesystem/network behavior unverified; not an OS sandbox or enforced resource quota |
| Versioned private handoff, REQ-COBS-005 | No portable observation manifest established | Format v1, digest recipes, sanitizer/privacy version and count/max-limit guidance in operator guide | Documentation only: no fixture producer, sanitizer, manifest validator or privacy certification implemented |
| Evidence boundaries, REQ-COBS-006 | Existing source and runtime facts are distinct | This matrix and private handoff instructions | No actual fixture, exact binary/UI identity, server run, browser history/route rendering, privacy review or capture artifact is qualified by this change |

The generic SQLite API and branch-specific work must not be described as a landed
end-to-end observation feature. The launcher depends on the server writer's
observation invocation; using an arbitrary existing release is not sufficient.

## Verification performed

- `python3 -m unittest discover -s tests/devpy -p test_observe.py -v`:
  **7 passed**, no child executable launched. Covers parser/defaults/rejections,
  no tracing/pretty/build bootstrap, exact argv, inherited-environment exclusion,
  temporary-directory cleanup, executable failures, signal forwarding/restoration,
  exit propagation and exception-time child reaping.
- The server admission, application-state and route-scope source was inspected.
  Its authored Rust tests were **not run** for the later closure. Source review
  caught a non-Copy CLI enum and anonymous capability bootstrap deadlock; both
  were corrected before `d3c756e98`. Parent review then caught missing ConnectInfo
  in the login regression; the source-only correction preserves all subsequent
  cookie/authentication assertions.
- Focused persisted-observation UI tests: **14 passed**, with scoped ESLint and
  TypeScript checking passed. The earlier broader ownership selection passed
  **231 tests**; these are not browser or Rust qualification receipts.
- New Rust-derived capability/target TypeScript files require actual hosted
  codegen output; no generated file was hand-authored.
- Full `./dev.py check`, Rust compilation/tests, binary startup and browser QA
  were deliberately **not run** under the no-build/no-binary-run constraint.

## Failed closure qualification and causal correction

Hosted run 39041380935 at exact source `cf61469e` terminated during library-test
compilation with 12 E0425 errors: mechanically renamed test calls incorrectly
qualified the project extractor as `axum::extract::LiveState`. No contract, startup,
codegen or candidate binary test from that run executed. The UI embed build passed;
it does not rescue the failed Rust gate. The causal child replaces all 12 calls
with `crate::api::application_state::LiveState` without changing assertions and adds
a negative source guard. The corrected child remains uncompiled until its separate
hosted qualification. Original failure logs remain retained by the integration owner.

## Next qualification boundary

Integrate the retained server source and this launcher without changing existing
stock services. On an authorized resource-qualified executor, select an already
qualified exact observation-capable binary and an independently authorized private
fixture. Check the manifest identities/privacy/limits, then exercise actual
admission, denied capabilities, scoped reads, shutdown and browser presentation.
Record each observed result separately. Fixture acquisition or sanitization is a
separate explicit operation, never a default effect of `observe`.
