# ADR-212: macOS Bash launch qualification is the only separate hosted launch CI

- **Status:** Accepted
- **Date:** 2026-11-01
- **Affects:** `specs/bash/requirements.md` REQ-BASH-017; supplements ADR-143

## Context

ADR-143 placed focused launch-fault regressions in `.github/workflows/bash-launch.yml` on hosted Linux and macOS, alongside a generated wire-contract artifact. The required CI rust lane now builds the same native launcher, requires it (`PHOENIX_BASH_LAUNCH_TEST_REQUIRED=1`) and runs those regressions in the full Linux suite, and its codegen-stale check rejects drifted generated contracts. The Linux job and the artifact therefore duplicated required CI. The macOS job compiled each regression filter as a separate `cargo test` invocation, cold, on every change to widely edited runtime files, taking 18–35 minutes per pull request.

macOS-only code (`process_start_time` via `proc_pidinfo`, platform helpers, launcher re-exec and close retirement) is compiled by no other CI. The bug class ADR-143 addresses originates there, and macOS is a shipped platform.

## Options considered

1. Delete the workflow: removes the only hosted compilation of macOS launch and identity code.
2. Keep the macOS job, run it only after merge: no PR latency, but a macOS regression lands before it is detected.
3. Keep the macOS job blocking pull requests, drop the duplicated Linux job and artifact, compile the regressions once, trigger only on files containing macOS launch or identity code, and seed its cache from main: selected.

## Decision

Option 3. The workflow runs one macOS job. Its regressions run as two nextest invocations over identical package and target selections, so the compile happens once; the runtime cases stay serial. It triggers on pull requests and main pushes touching `process_identity.rs`, `platform.rs`, `phoenix-tools` Bash sources, `phoenix-ide` `main.rs`, `runtime/close_retirement.rs`, or the workflow. Linux coverage of the same regressions is provided by required CI.

## Consequences

- macOS launch and identity code stays qualified before merge on the files that contain it.
- A change confined to shared runtime files that breaks only macOS launch behaviour is no longer qualified on macOS before merge; Linux CI still runs the same regressions.
- The workflow no longer publishes generated wire contracts; `./dev.py codegen` and CI's codegen-stale check own them.
