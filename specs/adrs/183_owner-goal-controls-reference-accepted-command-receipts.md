# ADR-183: Owner goal controls reference accepted command receipts

- **Status:** Accepted
- **Date:** 2026-10-20
- **Affects:** REQ-GOAL-003, REQ-GOAL-004/004.1, REQ-GOAL-007/008/009; extends ADR-182

## Context

The immutable-goal foundation stored real chat/runtime/host evidence but did not
have an attribution shape for an authenticated owner control that creates no
chat message. Using a synthetic message would misstate the cause. Reusing an
accepted Stop/release receipt would prove a different command. Actor labels and
principal UUIDs supplied in a payload were not application authorization.

Migration132 had already been imported and qualified as an immutable source.
Its history and execution-binding tables referenced each other, and history
referenced actor rows. A separately coordinated134 slot was reserved after a
federation133 slot, but actual133 was not available at this decision. Candidate
source work therefore could not register134 or modify132. The actor rebuild
needed to retain already committed locators even when original messages or
execution records had disappeared.

## Options considered

1. Fabricate a user-API message for each control. Rejected because message/source
   identity would no longer describe an actual input.
2. Broaden the actor kind and accept any existing control receipt. Rejected
   because Stop/release evidence cannot establish the actual goal command,
   target or observed/resulting version.
3. Retain command-specific receipts and bind UserControl attribution to the
   exact accepted mutation. Chosen, with independent authentication and a
   separately qualified schema upgrade.

## Decision

Use a UserControl actor containing the actual request/principal UUIDs and bounded
Create/State command kind. The principal is non-secret source correlation, not a
credential or permission. The application authenticates owner context and
reauthorizes the ordinary open owner before receipt lookup. New goal commands
use settled-boundary admission; exact retries preserve their own historical
result. No caller-supplied actor/principal or synthetic chat substitutes for this
boundary.

Persist goal-command receipts separately from Stop/release receipts. An accepted
receipt, its attributed goal history and immutable identity commit in one owning
transaction. Their owner, request, command, expected/result versions and goal
identity must agree. Accepted command input is reconstructed from its canonical
immutable goal/history rows; only rejected input needs separately retained scalar
payload. There is no second authoritative accepted objective or goal snapshot.
Goal commands do not clear or advance the independent Stop latch.

Bind UserControl actors to the exact accepted command and its resulting history
version. A direct owner-control request is both requested and applied through the
same causal receipt; requested/applied fields remain distinct for other actor
families. One actor row per request is reused for these direct roles, with a
deferred history reference requiring it in both slots of that command's row.
Receipt insert-conflict guards and history/actor reference checks prevent SQLite
REPLACE from changing retained evidence without relying on recursive triggers or
an undocumented automatic-rowid value.

Prepare the actor upgrade by staging exact actor/history/binding rows in one
transaction with deferred foreign-key checking, evacuating the populated cycle,
recreating actors under their original table name, and restoring retained rows
before reinstating source/admission validators. Do not rename the old parent and
risk retargeting dependent foreign keys; do not revalidate historical locators as
new admissions. Preserve identity/objective/history/binding values and normal
owner-deletion semantics. Keep this candidate unregistered until actual133 is
inspected and its supported populated upgrade is qualified. This records a
candidate architecture, not execution evidence, activation or a new cross-version,
rollback, live-database replacement or recovery guarantee.

## Consequences

- A typed UserControl actor does not by itself install an authenticated mutation
  service, endpoint, tool or UI. Exact-head source execution and the actual
  application boundary remain separately required.
- Legacy132 read/write branches do not query candidate-only fields. Their SQL
  source and registry identity stay unchanged.
- Fresh and populated-cycle, source-deletion, rollback/fault, wrong-command,
  rejected-receipt, owner/principal/request/version and hard-delete tests are
  required. Source tests on132 alone do not qualify actual133→134.
- New admission/report and model/tool checks remain default-disabled until
  genuine runtime integration is qualified. Pre-dispatch database checks are not
  strict Stop/send linearization or cancellation of work already dispatched.
- No goal state, actor receipt or new identity releases Stop, grants resources,
  replaces task approval, or changes the sole task-approval proposal pathway.
