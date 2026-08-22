`specs/browser-tool/browser-lifecycle.allium` does not parse: the
`LifecycleEventFannedOut` rule references `SseEvent.BrowserSessionState`
through an import alias the file never declares, so `allium check` reports
`allium.reference.undefinedImportedAlias`. Present since #633.

The repo-wide allium gate only surfaces this when `allium-cli` is installed
(`./dev.py check` skips the lane otherwise), which is why CI is green.

The rule's postcondition asserts that each live conversation resolving to
the scope receives the `browser_session_state` SSE event with kind `active`
or `teardown_pending`. A faithful fix has to keep that subject — the SSE
event delivered over the wire — rather than substituting the internal
`BrowserSessionLifecycleEvent`, which a fan-out could satisfy without ever
sending the SSE state. Either import `../sse_wire/sse_wire.allium` and
reference a per-variant value there, or declare the SSE event locally;
`sse_wire` currently models carrier kinds as string discriminants rather
than referencable values, so that choice is a modelling decision for the
browser-tool spec owner.

Note also that `kind = active or kind = teardown_pending` supplies a boolean
expression to an enum-valued `kind` field; a set-membership form expresses
the intent.

## Resolution

Fixed by declaring `BrowserSessionStateEvent` locally as the wire-visible
counterpart of the internal lifecycle event, and asserting delivery of that
event with `kind in { active, teardown_pending }`. The postcondition's
subject is preserved — fanning out the internal event is explicitly not
delivery — and the enum-valued `kind` no longer receives a boolean
expression.

Importing a per-variant value from specs/sse_wire remains preferable and is
still open as a modelling question there: sse_wire models carrier kinds as
string discriminants, so there is nothing to reference yet.
