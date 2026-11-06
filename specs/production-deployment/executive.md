# Cross-Platform Production Deployment — Executive Status

## Current reality

Backend selection is native launchd on macOS, systemd on Linux with systemd, and
the persistent same-user supervisor otherwise. Exact checked local HEAD and
checksummed published releases remain ordinary sources. Candidate identity is a
complete 40-character SHA and exact package version; activation belongs to an
independent backend owner and durable status remains authoritative.

macOS now uses protocol 2 and candidate-owned migration once, with failed
ownership/evidence retained for manual recovery after disruption. The launchd
executive and ADR-230 account for the withdrawn prepared, paired, current-schema,
refresh, repair, rollback, finalization and replay contracts and their tests.
Installed-state restart remains unchanged. Active macOS deploy/restart markers
always block a new operation until owner release or explicit operator resolution.

Linux ordinary runtime-artifact rollback is unchanged. Exact protected systemd
published/prepared admission under REQ-PD-023–027/029 retains fixed candidate and
registry/receipt pins, preserved installed configuration, persistent both-unit
inhibits, offline pristine backup and isolated rehearsal, durable live marker and
candidate-only post-marker recovery. Ordinary systemd still rejects pending
migrations before handoff. Bare Linux retains authenticated supervisor ownership,
startup digest matching, direct-child identity, stop semantics and reboot guidance.

## Requirement coverage

| Requirements | Current implementation / verification |
| --- | --- |
| REQ-PD-001–009,011–017 | Backend selection/source preparation, independent ownership, exact verification and durable status; backend-specific tests and disposable harnesses |
| REQ-PD-010 | Ordinary Linux runtime-artifact rollback; no database compatibility claim |
| REQ-PD-021,023–027,029 | Existing exact protected Linux contracts preserved; systemd focused suite 32 tests passes |
| REQ-PD-018–020,022,028,030 | Withdrawn macOS routes; ADR-230 |
| REQ-PD-031 | Protocol-2 launchd helper, 21 helper and 60 installer tests; explicit manual recovery and 32 UI tests |

Reported focused Linux results are systemd 32 and bare 21 tests passing. macOS
helper/installer/restart/harness evidence and explicit test-retirement accounting
are in `specs/launchd-deployment/executive.md`. No new source was deployed;
production remains `9c5d471d`. These focused results do not substitute for full or
hosted qualification.

## Operator surface

Ordinary commands are `prod deploy`, `prod deploy --release TAG|latest`,
`prod status` and `prod stop`. Positional versions and `prod set`/`prod unset`
reject with configuration guidance. macOS restart preserves installed state;
bare Linux restart snapshots configuration through its supervisor; systemd has
no `prod restart` surface. The exact protected Linux modes and
`prod recover-systemd-current TXN` retain their existing requirements and
`SYSTEMD.md` guidance. There are no macOS schema-specific recovery commands.
