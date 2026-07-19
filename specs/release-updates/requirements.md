# In-App Release Updates Requirements

## Scope

In-app discovery, preview, approval, installation tracking, and operator messaging for stable published Phoenix releases. This specification defines the user-facing contract above `specs/production-deployment/`: it decides which published release is offered, how the preview remains immutable, how explicit approval binds to an exact release identity on the same host, how the UI reconnects to durable backend progress, and how success or rollback is reported truthfully. It does not redefine backend activation, exact runtime verification, or rollback mechanics, which remain owned by `specs/production-deployment/requirements.md`.

## Requirements

### REQ-RU-001 — Stable published release discovery

The system shall discover in-app update candidates only from stable published releases. Pre-release, draft, locally built, or unverified artifacts shall not appear as installable in-app updates.

### REQ-RU-002 — One immutable preview per discovered release

When the system offers an update, it shall create an immutable preview that resolves one stable published release to one tag, one full source commit, one host-target asset, and one release summary visible to the operator. The install action shall refer to that exact preview rather than re-resolving release identity at click time.

### REQ-RU-003 — Explicit same-host approval bound to exact identity

The system shall allow installation approval only from a same-host browser session and shall bind that approval to the preview's exact release tag and full source commit. If the offered release identity changes or the preview becomes stale, the system shall require a fresh preview and a fresh approval.

### REQ-RU-004 — Production deployment remains the installation authority

After approval, the system shall request installation through the production-deployment contract for published releases and shall not introduce a parallel activation, verification, or rollback path for in-app updates.

### REQ-RU-004A — Runtime ownership gates backend eligibility

The system shall derive the release-update backend from the authoritative
runtime-ownership snapshot defined by `specs/deployment-info/requirements.md`
and shall not independently infer ownership from the host platform, PID 1, or
installed artifacts. Development, unmanaged, ambiguous, and unsupported
ownership states shall not be eligible for disruptive in-app updates.
The system shall pass the proven managed backend to the independent controller,
and the controller shall deploy through that backend rather than re-inferring one
from host characteristics.

Browser locality, host tools, privileges, and deployment-claim availability
shall remain separate update-authority decisions and shall not alter the
runtime-ownership snapshot.

### REQ-RU-005 — Independent controller and backend lifecycles

The in-app release-update controller shall remain independent from the backend installation lifecycle. Closing the dialog, navigating away, disconnecting, or restarting Phoenix shall not cancel or complete installation by implication.

### REQ-RU-006 — Durable status hydration after reconnect

When a same conversation or later session reconnects after disconnect, reload, or Phoenix restart, the release-update controller shall hydrate from durable backend status for the approved installation attempt and present the latest known nonterminal or terminal outcome without requiring the user to remember prior UI state.

### REQ-RU-007 — Truthful terminal success language

The in-app update surface shall present installation as successful only when the backend has durably recorded a committed published-release deployment whose exact runtime identity matches the approved tag and full source commit.

### REQ-RU-008 — Truthful rollback and failure language

If the backend records rollback, rollback failure, rejection, or another non-success terminal outcome, the in-app update surface shall say so explicitly and shall not imply that the attempted release became active.

### REQ-RU-009 — Terminal and replay-safe audit trail

The system shall preserve enough durable release-update identity to explain which preview was approved, which published tag and full commit were attempted, and which backend terminal outcome was observed after reconnect.

### REQ-RU-010 — No checkout or operator CLI dependency

The in-app release-update capability shall not require a source checkout or an operator-invoked deployment command. Phoenix may internally materialize and execute a source-pinned deployment controller artifact from the approved release so the established cross-platform activation engines remain authoritative. The repository `./dev.py` command remains a local bootstrap, development, offline repair, migration, and emergency-recovery entrypoint.
