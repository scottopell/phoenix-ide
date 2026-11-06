# Standalone Deployment Controller — Executive Status

The standalone Rust controller is retired under ADR-229. REQ-SDC-001–023 remain
withdrawn tombstones and its behavioral Allium file is removed. The crate had no
production caller; removing it changes no production orchestration.

Its 54 files (about 51,559 lines), including its own policy/store/native guardian,
IPC, recovery, fixture and regression tests, are intentionally removed with the
withdrawn contracts. The dedicated macOS workflow, prepared source114 artifact
fixture and `launchd_validation_harness.py`/README are retired with it. This is
contract retirement, not deletion of active regressions to make checks green.
The ordinary disposable launchd harness and existing restart helper remain.

Routine macOS deployment instead uses the small protocol-2 Python helper and
operator recovery after disruption. Targeted replacement coverage is documented
in `specs/launchd-deployment/executive.md`; Linux contracts remain unchanged.
Historical ADR-133–141 and historical transaction files remain evidence only,
without an adoption or replay requirement. No deployment occurred in this change.
