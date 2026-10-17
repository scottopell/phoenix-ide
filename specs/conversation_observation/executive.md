# Conversation observation — current evidence

## Status

The reusable `dev.py observe` launcher and operator guide are implemented as
uncommitted source in the shared worktree. Seven focused Python unit tests pass
with the child process mocked. No Phoenix binary was built or run for this slice;
no fixture was acquired/created and no browser/server acceptance is claimed.
The existing stock service on port 18043 was not contacted or altered.

Roadmap issue #806 was read. The local `origin/main` reference used for this
bounded source audit was `d07417325`; it was not refreshed, so this is not a claim
about newer remote main or deployment. Worktree HEAD was `196fc1e3d` on
`bench/current-readonly-history-compat`. Concurrent retained application-state and
observation-startup edits were inspected, not owned or validated by this slice.

## Landed versus retained gap matrix

| Capability / requirement | Landed baseline evidence | Retained/worktree evidence | Gap / acceptance still owed |
|---|---|---|---|
| Read-only SQLite primitive, REQ-COBS-003 | Local `origin/main` contains `Database::open_read_only` in `crates/phoenix-db/src/lib.rs` | Startup calls this primitive after standalone admission | No binary qualification here; read-only primitive alone does not prove isolation or schema sufficiency |
| Shared typed persisted history projections, REQ-COBS-004 | Not established as landed by this bounded audit | HEAD `196fc1e3d`, `refactor: share typed persisted history projections without live state` | Retained branch implementation, not proof of deployment or UI parity |
| Immutable fixture and scoped observation server, REQ-COBS-003/004 | Local `origin/main` CLI has no observation invocation; `api/application_state.rs` absent | Uncommitted `main.rs`, `immutable_fixture::standalone_path`, `observation_startup::run`, `ObservationApplication`, scoped `ConversationRouteObservations` and authored startup tests | Server owner must integrate/review and qualify exact binary: admission/denial, no migration/writes/live services, target scope, loopback/forwarding, startup/shutdown and supported schemas |
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
  Its authored Rust tests were **not run** in this slice.
- Full `./dev.py check`, Rust compilation/tests, binary startup and browser QA
  were deliberately **not run** under the no-build/no-binary-run constraint.

## Next qualification boundary

Integrate the retained server source and this launcher without changing existing
stock services. On an authorized resource-qualified executor, select an already
qualified exact observation-capable binary and an independently authorized private
fixture. Check the manifest identities/privacy/limits, then exercise actual
admission, denied capabilities, scoped reads, shutdown and browser presentation.
Record each observed result separately. Fixture acquisition or sanitization is a
separate explicit operation, never a default effect of `observe`.
