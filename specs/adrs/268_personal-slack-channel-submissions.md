# ADR-268: Accept personal-channel submissions through the Slack gateway

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-SG-002/005/007/009/011/016/017/020

## Context

Saved capture is private but cannot attach an investigation instruction. The owner
wants to forward a Slack message with a note describing the desired perspective.
A controlled read-only connector check exposed the note, forwarded source link
and quoted preview separately in an exact message read; search exposed only the
note. The source discussion was independently readable. Private sample identities
and content remain outside the public repository.

## Decision

Add one optional submission-channel ID to the existing gateway. A new owner-authored
top-level post is the invocation. No emoji or forwarding-specific gesture is needed.
The owner's note supplies the instruction; the forwarded block is quoted context.
An absent note requests a briefing, never execution of the forwarded text.

Use the submission message's identity and thread for acceptance and result delivery.
Each top-level post creates a fresh native conversation; subsequent interaction is
in Phoenix. Return a Phoenix link with the intentional answer. Do not consume plain
thread replies as instructions or automatically publish to the original discussion.
Retain ordinary permission and clarification behavior.

Reuse native conversation creation, durable inputs, request-bound results and the
gateway's worker and allowance. Keep channel initialization and progress separate
from emoji and Saved scans. Use the four-calendar-date submission window; original
messages can be any age. Emoji and submission discovery share source deduplication;
Saved capture remains independent. No generic adapter framework is introduced.

## Options considered

- A prefix in self-DM reuses the emoji workflow but retains the extra gesture.
- A full private Slack conversation adds follow-up discovery and question semantics
  beyond the approved submission-and-Phoenix-handoff workflow.
- A separate relay process duplicates ownership and persistence without a need.

## Consequences

The feature adds channel-scoped discovery, note parsing, retained initialization
and owner-facing status. A source preview alone is not proof that the original
discussion was read. Private-channel membership and visibility remain Slack-managed.
Replies sent as the owner may not notify the owner; native Phoenix remains the
authoritative place to find the work. No deployment or live activation is implied.
