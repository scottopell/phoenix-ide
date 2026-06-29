# Writing EARS Requirements

EARS — Easy Approach to Requirements Syntax — was developed at Rolls-Royce for
aviation systems. It gives requirements a small, consistent grammar that keeps
them unambiguous and testable: each statement translates directly to a test.
This guide covers the grammar, the ID and title conventions, and the one mistake
that causes the most pain — leaking implementation into requirements.

Requirements are **timeless**: they state what must be true, independent of
whether it is built yet.
An unimplemented requirement is still a valid requirement — it just has a `❌` in
`executive.md`. Keep status, implementation, and decision-logs *out* of
`requirements.md` entirely; they live in `executive.md`, the code, and ADRs
respectively.

## The grammar

Every requirement is one or more clauses of:

```text
<condition> THE SYSTEM SHALL <observable behavior>
```

There are five patterns, distinguished by their condition keyword.

**1. Ubiquitous** — always true, no trigger:

```text
THE SYSTEM SHALL validate email format before account creation
```

**2. Event-driven** (`WHEN`) — a state change or action triggers behavior:

```text
WHEN a user submits the form
THE SYSTEM SHALL validate every required field
```

**3. State-driven** (`WHILE`) — behavior holds during a condition:

```text
WHILE a request queue is full
THE SYSTEM SHALL return 503 Service Unavailable
```

**4. Unwanted behavior** (`IF … THEN`) — explicit prohibition or error handling:

```text
IF an authentication token is invalid
THE SYSTEM SHALL NOT return account data
```

**5. Optional feature** (`WHERE`) — behavior gated on a configuration:

```text
WHERE extended analysis is enabled
THE SYSTEM SHALL include the deep-scan section in the report
```

Most requirements pair a happy path with its edge cases — several `WHEN`/`IF`
clauses under one requirement.
One clause should map to one test.

## Specific and measurable, not vague

A requirement a reader could interpret two ways is too vague; one that names a
technology is too detailed.
Aim for observable, verifiable behavior with concrete criteria.

| Avoid | Prefer |
| --- | --- |
| THE SYSTEM SHALL be fast | WHEN data is cached, THE SYSTEM SHALL respond within 2 seconds |
| THE SYSTEM SHALL handle errors gracefully | WHEN the database connection fails, THE SYSTEM SHALL display a retry message |
| THE SYSTEM SHALL provide good UX | WHEN form validation fails, THE SYSTEM SHALL highlight the invalid fields |

Good test: could someone else implement this from the requirement alone, and
could you write a test that passes or fails unambiguously?
If not, sharpen it.

## Requirement IDs

Every requirement gets an immutable ID: `REQ-<ABBREV>-###`.

- `<ABBREV>` — a short feature abbreviation (`RL` rate limiting, `UA` user auth,
  `TA` task approval).
- `###` — zero-padded sequential number within the feature.
- **IDs are never reused or renumbered.** This is the load-bearing rule of the
  whole system: the ID is what makes a requirement greppable across specs,
  tests, code, and ADRs.
  Change the EARS text freely (git shows the history); never change the ID.

To deprecate a requirement, do not delete it — mark it and back it with an ADR:

```markdown
### REQ-RL-003: [Original Title]

**DEPRECATED:** Replaced by REQ-RL-008. See ADR-014.

[Original EARS statements preserved]
```

## Titles describe user benefit, not implementation

The title sets the tone.
A title that names a mechanism poisons the requirement toward implementation
detail; a title that names a user benefit keeps it honest.

| Implementation-focused (avoid) | User-benefit (prefer) |
| --- | --- |
| Viewport-Based Query | Discover Activity in a Region |
| IP-Based Rate Limiting | Prevent Abuse Attacks |
| Redis Cache Integration | Instant Response for Repeat Visits |
| JWT Token Validation | Secure User Sessions |

Red flag: a title ending in “-ing” (Caching, Processing, Querying) is almost
always describing a mechanism.
Start titles with a user-facing verb — Discover, View, Prevent, Secure, Enable.

## Rationale answers “why does the user care?”

Every requirement carries a rationale, and it must answer one question: *why
does the user care?* (equivalently, *what value does this give them?*). spEARS
emphasizes user-facing value over technical concern.

```text
✅ "Users want to scan across regions without waiting. Fast response keeps exploration
    fluid; slow responses would discourage browsing entirely."

❌ "Enables spatial discovery of cached data. The 500ms target ensures responsive
    interaction. WGS84 is the standard coordinate system."
```

The second is all mechanism — it belongs to an ADR or the Allium spec, not the
rationale.

## The implementation leak (most common failure)

Requirements describe *what the user observes*, never *how it is built*.
Technology names, data-structure fields, and algorithms all belong elsewhere
(the `.allium` spec, the code, or an ADR). This is the single most frequent and
damaging mistake.

**Technology / infrastructure:**

```text
❌ THE SYSTEM SHALL use Redis for caching
✅ THE SYSTEM SHALL retain cached data across server restarts

❌ THE SYSTEM SHALL store sessions in the user_sessions table
✅ THE SYSTEM SHALL keep a user signed in across browser refreshes
```

**Data-structure field names:**

```text
❌ THE SYSTEM SHALL return user_id, created_at, and status fields
✅ THE SYSTEM SHALL show the account's identifier, creation date, and current status

❌ WHEN the is_active flag is true
✅ WHEN the user's account is active
```

**Algorithms:**

```text
❌ WHEN a viewport query is received, THE SYSTEM SHALL complete within 500ms using
   geohash prefix queries
✅ WHEN a user pans or zooms, THE SYSTEM SHALL update displayed activity within 500ms
```

The performance *target* is a fine requirement; the *algorithm* that meets it is
not. If the requirement names a thing a user could never see, move that detail
out.

## Quick checklist

Before committing a requirement:

- [ ] Title names a user benefit and starts with a user-facing verb
- [ ] Each clause uses an EARS pattern (`WHEN` / `WHILE` / `IF` / `WHERE` /
  ubiquitous)
- [ ] Behavior is observable and testable; criteria are specific
- [ ] No technology, field names, or algorithms
- [ ] Rationale answers “why does the user care?”
- [ ] ID is new and sequential — never reused
- [ ] No status, implementation, or dates (those live elsewhere)
