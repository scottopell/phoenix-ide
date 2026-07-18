# Authoritative installation ownership model

## User outcome

About this deployment and the in-app updater describe how the running Phoenix instance is actually owned, instead of inferring an installation technique from the host operating system.

## Scope

- Extend the deployment-info specification with one shared typed runtime-ownership contract; update production-deployment and release-updates only by reference where their existing authority boundaries require it.
- Introduce a closed Rust model that distinguishes proven `launchd_managed`, `systemd_managed`, `bare_supervisor_managed`, `development`, `unmanaged`, `ambiguous`, and `unsupported` states. Refine names during implementation if evidence shows a smaller correct type.
- Detect ownership from runtime contracts and backend-owned evidence, with socket/service/supervisor ownership outranking filesystem hints. Host capability such as macOS or PID 1 being systemd is not sufficient proof by itself.
- Represent contradictory or insufficient evidence explicitly; never silently default an unknown Linux process to bare supervision or any macOS process to launchd.
- Keep browser locality and update prerequisites separate from installation ownership.
- Expose the same typed snapshot through `/api/deployment`; derive release-update backend eligibility from it rather than maintaining a second detector.
- Add cross-platform unit/API tests for precedence, ambiguity, unmanaged development/manual runs, and update-authority derivation.

## Acceptance criteria

- [ ] About and release updates consume one authoritative ownership value.
- [ ] A manually launched macOS binary is not labeled launchd-managed.
- [ ] A non-systemd Phoenix process on a systemd-capable Linux host is not labeled systemd-managed.
- [ ] A live compatible bare supervisor is positively identified from owner evidence.
- [ ] Ambiguous/unreadable evidence remains typed and visible rather than guessed.
- [ ] Local/remote browser status cannot alter installation ownership.
- [ ] Existing launchd, systemd, bare, development, and musl checks pass.
