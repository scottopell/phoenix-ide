# ADR-217: Coordinator Authority uses conversational approval and direct tools

- **Status:** Accepted
- **Date:** 2026-11-04
- **Affects:** REQ-AUTH-001 through REQ-AUTH-006, REQ-GR-007, REQ-GR-013, REQ-GR-022

## Context

The user wants Ember to retain standing approvals and operating boundaries so
routine authorized work does not repeatedly return for permission. Production
conversation evidence included blanket cache-cleanup approval that Ember used,
followed by later individual cleanup approval packets. Reconstructing authority
from summaries and mixed historical evidence leaves its effective scope unclear.

The initial exploration joined this need to durable approval proposals, inline
review, independently pending questions and optional timeout continuation. The
user explicitly separated those concerns as scope creep and requested Authority
tools that rely on the agent obtaining approval before calling the setter.

## Options considered

1. **Retain Authority only in transcript summaries or home notes.** Small, but
   summaries are lossy and notes are historical evidence rather than a current
   instruction source. It leaves the observed confidence problem intact.
2. **Persist proposals and require a server-verified human approval action.**
   Binds exact reviewed text, but introduces request identity, pending lifecycle,
   client review actions and activation/delivery semantics. Adding late answers
   or timed waiting further crosses scheduling, recovery and Stop boundaries.
3. **Persist one current document and use direct Coordinator tools with
   conversational approval.** Supplies durable current instructions while
   reusing ordinary conversation for clarification and approval. It deliberately
   relies on model behavior for obtaining human permission.

## Decision

Choose option 3. Provide local `read_authority` and `set_authority` tools only to
the current Coordinator. The setter replaces the complete text, with empty text
clearing the document and an explicit 16,384-byte UTF-8 bound. Store it as one
indivisible scalar value in `app_settings`; no migration or versioned proposal
store is needed. Bind source identity through tool execution context and
revalidate current Coordinator identity in the committing write transaction.

Instruct Ember to interpret user feedback into clear scope, clarify material
ambiguity, present the complete replacement and obtain approval before saving.
There is no caller-supplied approval flag or server approval proof. Supply the
saved text through authoritative context on every ordinary Coordinator request,
including after continuation; use the same renderer for prompt inspection.

The setter follows existing local write ownership and outcome classification.
That persistence correctness is distinct from proving human consent and does
not require a new durable workflow. Existing action restrictions remain binding;
Authority is model guidance, not a semantic Bash policy evaluator.

## Consequences

- **Positive:** Current authority survives continuation without reconstruction
  from model-generated summaries. Scope can be clarified in normal conversation.
- **Positive:** No approval UI, pending-proposal lifecycle, version protocol,
  timer, scheduler or changes to `ask_user_question` are required for delivery.
- **Positive:** The scalar document reuses an existing persistence mechanism and
  keeps the tool API small and inspectable.
- **Negative:** The server does not prove that a human approved a write. A model
  can violate its instruction; the product must not advertise a hard consent
  guarantee or semantic action enforcement.
- **Negative:** Full replacement has no revision conflict protocol or retained
  document history beyond ordinary tool/transcript records. The agent must read
  current text and preserve relevant boundaries when composing a replacement.
- **Neutral:** Independently durable human interactions and timeout behavior
  remain a separate design problem, without a commitment to build them.

## References

- [Coordinator Authority requirements](../coordinator-authority/requirements.md)
- [Coordinator requirements](../global-recall/requirements.md)
- [Permissions requirements](../permissions/requirements.md)
- [Compatibility requirements](../compatibility/requirements.md)
- ADR-066: on-demand activity and stable Coordinator request prefixes.
- ADR-070: trusted input provenance does not prove human authorship.
- ADR-098: retired wake execution does not commission a replacement scheduler.
- ADR-101: Coordinator home files are retained evidence, not authority.
