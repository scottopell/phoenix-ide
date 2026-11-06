# Federate automatic-continuation settings

Extend the existing native `read_automatic_continuation` and `set_automatic_continuation` tools with exact saved-peer routing. Preserve their closed target/value contract, current-Coordinator authority, destination-owned validation, prospective non-dispatch semantics, explicit ambiguity/not-found failures, and no local fallback. Add authenticated Federation peer endpoints/client routing, normative Federation coverage, and focused regressions. Do not add a combined enable-and-create operation or mutate trust/enrollment.

## Completion criteria

- Omitted `instance` and exact `"local"` retain local behavior; another non-null selector resolves exactly one saved peer by canonical UUID or unique saved name.
- Unknown, ambiguous, and self selectors fail without local or alternate-peer fallback.
- Trusted ToolUse and current-Coordinator authority are required at dispatch; source authority and the ProductConversation admission gate remain held through the bounded peer request even if the requester is cancelled.
- Closing source or destination local authority suppresses ordinary result publication.
- Read and set use distinct closed request types; set requires a strict Boolean and rejects missing, null, string, and numeric values.
- The destination authenticates exact caller and destination identities and transactionally rechecks Federation send authority while resolving and reading or writing the destination-owned target.
- Responses validate exact caller, destination, target kind, exact ordinary target identity, authenticated-destination Coordinator identity semantics, and the set Boolean.
- An unclassified write, panic, or unexpected worker exit closes destination authority before unresolved write ownership is released and does not become an ordinary retryable failure.
- Remote set is prospective: it does not create, admit, wake, dispatch, retry, rename, continue, alter Goals, or mutate trust/enrollment.
- Focused wire, selector, cancellation, authority, destination roundtrip, persistence, and side-effect regressions pass; owning-crate strict Clippy and the repository broad check pass.
- Federation and Global Recall requirements, Allium, and executive status agree with the implementation; final independent review has no unresolved actionable finding.
