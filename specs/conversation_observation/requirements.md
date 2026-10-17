# Conversation observation requirements

## Purpose and boundary

Provide a repeatable way to inspect persisted conversation behavior without
activating live conversation execution or modifying the supplied fixture.
Observation is not fixture acquisition, sanitization, schema migration, provider
execution, production administration or an operating-system sandbox.

### REQ-COBS-001: Explicit no-build launch

WHEN an operator invokes `dev.py observe`, THE SYSTEM SHALL require an absolute
fixture path, a nonempty conversation reference and an explicitly selected existing
executable, and SHALL pass them to that executable's `--observe-fixture` invocation.
THE SYSTEM SHALL NOT implicitly build, seed, acquire data, migrate, start Vite,
bootstrap development tracing/pretty dependencies, load provider configuration or
fall back to ordinary server startup. An unavailable executable or invalid argument
SHALL fail before launching a child.

### REQ-COBS-002: Isolated invocation

THE launcher SHALL provide a private temporary working directory, HOME, XDG
config/data/cache/state and temporary directory. THE child environment SHALL contain
only those directory variables and fixed locale settings, not inherited provider,
production, proxy, tracing or loader configuration. THE launcher SHALL run in the
foreground, forward SIGINT/SIGTERM, propagate child failure and remove scratch
directories after child exit. THE launcher SHALL NOT manage development or
production service state. The default port SHALL be OS-selected; an occupied
explicit port SHALL NOT authorize terminating its owner.

### REQ-COBS-003: Authoritative immutable fixture admission

THE observation server SHALL be the authority for standalone fixture and target
admission. THE launcher SHALL NOT open, chmod, copy, repair, checkpoint or remove
sidecars from the fixture. THE server SHALL reject nonabsolute/nonregular,
writable, hard-linked, production-like canonical locations and fixtures with
SQLite WAL, SHM or journal sidecars at the supplied or canonical path. THE server
SHALL open the fixture read-only without migrations and SHALL fail unsupported
required reads or unresolved targets rather than repair them or activate live
services. Filesystem admission SHALL NOT be represented as privacy verification.

### REQ-COBS-004: Target-scoped observation authority

THE server SHALL expose supported persisted observations scoped to the admitted
ordinary or coordinator ProductConversation and SHALL NOT construct live runtime
services. Unsupported live capabilities SHALL remain unavailable. THE listener
SHALL bind loopback, reject forwarded access and print its actual bound address
only after admission succeeds. This local unauthenticated authority SHALL NOT be
represented as authorization to publish a proxy or tunnel.

### REQ-COBS-005: Portable, privacy-aware handoff

A shared fixture handoff SHALL carry the version-1 manifest described in
[the operator guide](../../docs/conversation-observation.md), recording exact fixture,
schema/ledger, source, binary, independent UI, sanitizer/version/rules, privacy
review, targets, counts and explicit maximum resource limits. Unavailable optional
identities SHALL be null rather than inferred. Portable metadata SHALL NOT contain
personal paths, hostnames, credentials or raw source conversation content. A
manifest SHALL NOT substitute for server admission, sanitization review or measured
acceptance. Manifest generation/validation and resource quotas are producer/operator
responsibilities, not implicit launcher operations.

### REQ-COBS-006: Evidence boundaries

Documentation and handoffs SHALL distinguish landed source, retained branch/source
work, executed unit tests, binary/server qualification and observed browser behavior.
THE SYSTEM SHALL NOT claim fixture privacy, frontend identity, runtime isolation or
UI acceptance solely from a source diff, manifest or mocked launcher test.
