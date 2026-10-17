# ADR-123: Retain pre-session browser authority and distinguish CLI profile lifetimes

Status: Accepted
Affects: browser session lifecycle; standalone browser CLI

## Context

Initialization previously returned a `BrowserSession` only after Chrome launch, marker installation, and first-page selection. Cleanup failures before that return could discard the local browser/group handles before the manager inserted a registry entry. Reusing a profile after such an uncertain launch is unsafe; refusing the profile without retaining the launch also prevents an authoritative retry.

The IDE's scope-derived profile contract preserves cookies and continuation ownership. The CLI daemon instead owns an explicit standalone lifetime. A new CLI daemon already has a UUID scope, so a permanent cross-daemon poisoning claim is not supported. Within one daemon, unsuccessful initialization and repeated browser lifetimes still need an explicit ownership and profile policy.

## Options considered

1. Allocate another profile whenever launch fails. Rejected: a different path does not stop or discharge an owned or unobservable earlier launch.
2. Return only a richer launch error. Rejected: an error string or tag cannot retain the actual child, handler, and affine group authority.
3. Reserve one manager-owned typed slot before potentially spawning, retain partial resources in it, and select a CLI-only transient profile policy. Chosen.
4. Fork the browser library or introduce a lower-level process supervisor. Not part of this decision.

## Decision

The existing registry, scope/actor keys, retirement generations, and close-attempt protocol own a launch reservation before any potentially spawning call. Its typed state distinguishes known pre-spawn failure, a retained partial browser, a ready session, an unobservable attempt, and authoritative stop. Initialization and one-time listener setup belong to the retained driver, not the requesting future. Exact permits match the immutable reservation even before a process identity is available.

Returned browser handles retain their actual child, handler, adopted group authority, profile path, and marker state across failed cleanup. Close retries that retained aggregate. Failure to adopt does not grant permission to reconstruct group authority from an old PID or marker.

A library launch error or timeout without a returned recoverable child is unobservable, not proof that no child exists. The slot remains a fail-closed barrier: no automatic fallback, fresh-path bypass, profile deletion, or successful absence report is authorized. This decision does not claim that the library returns or retains every child handle on opaque failure. Known failures before a potentially spawning call are distinguished and may retire without deleting untrusted profiles.

All IDE/default managers keep the existing scope-derived path. The standalone CLI explicitly selects a transient path for a genuinely fresh or authoritatively retired browser lifetime. Cleanup records and deletes the actual path, not a path re-derived from the scope. Existing untrusted paths remain untouched. Neither a failed owned teardown nor an unobservable attempt permits another lifetime.

## Consequences

- Caller cancellation cannot erase a manager-owned launch reservation or skip one-time initialization.
- Partial initialization remains visible to retirement and close retries instead of appearing absent.
- Opaque launch errors can require external intervention; this is an explicit conservative limitation, not a fallback-availability fix.
- Deterministic injected registry/cleanup tests are authored to exercise reservation and retry behavior, pending execution; they do not establish native child/group correctness. Native and real-Chrome qualification remain separate evidence.
- No dependency fork, independent authority map, old-profile adoption, or IDE profile-policy change is introduced.

## References

- `../browser-cli/requirements.md` REQ-BCLI-004, REQ-BCLI-006, REQ-BCLI-006a
- `../browser-tool/requirements.md` REQ-BROWSER-000, REQ-BROWSER-WS-001
- `../compatibility/requirements.md`
