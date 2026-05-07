# EARS Format & Spec Writing Guide

This reference covers the EARS syntax patterns, document templates, and the
quality checklists to apply when writing or updating spec documents.

## EARS Patterns

EARS (Easy Approach to Requirements Syntax) provides five patterns for writing
unambiguous, testable requirements.

### 1. Ubiquitous (Always True)

```text
THE SYSTEM SHALL validate email format before account creation
THE SYSTEM SHALL encrypt passwords using bcrypt
```

### 2. Event-Driven (State Changes)

```text
WHEN user clicks "Submit" button
THE SYSTEM SHALL validate form fields

WHEN API returns 500 error
THE SYSTEM SHALL retry request up to 3 times
```

### 3. State-Driven (Conditional Behavior)

```text
WHILE user is authenticated
THE SYSTEM SHALL display logout button

WHILE request queue is full
THE SYSTEM SHALL return 503 Service Unavailable
```

### 4. Unwanted Behavior (Explicit Prohibitions)

```text
IF user quota is exhausted
THE SYSTEM SHALL NOT process new requests

IF authentication token is invalid
THE SYSTEM SHALL NOT return sensitive data
```

### 5. Optional Features (Configurable)

```text
WHERE cache is enabled
THE SYSTEM SHALL return cached data within 100ms

WHERE premium mode is active
THE SYSTEM SHALL include extended analysis
```

## Writing Good EARS Requirements

### Specific and Measurable

```text
WHEN user makes 11th request from same IP within 1 hour
THE SYSTEM SHALL return HTTP 429 with X-RateLimit-Remaining: 0 header
```

### Error Conditions Explicit

```text
WHEN external API returns 503 error
THE SYSTEM SHALL display "Service temporarily unavailable" message
```

### Edge Cases Covered

```text
WHEN rate limit resets at hour boundary
THE SYSTEM SHALL allow new requests from previously blocked IPs
```

### What Makes a Bad Requirement

| Problem | Bad | Good |
| --- | --- | --- |
| Vague | “THE SYSTEM SHALL be fast” | “THE SYSTEM SHALL respond within 2 seconds for cached data” |
| Ambiguous | “THE SYSTEM SHALL handle errors gracefully” | “WHEN database connection fails, THE SYSTEM SHALL display retry message” |
| Not testable | “THE SYSTEM SHALL provide good user experience” | “WHEN form validation fails, THE SYSTEM SHALL highlight invalid fields” |
| Implementation detail | “THE SYSTEM SHALL use Redis for caching” | “THE SYSTEM SHALL persist cache across server restarts” |

* * *

## Document Templates

### requirements.md

```markdown
# [Feature Name]

## User Story

As a [user type], I need to [capability] so that [benefit].

## Requirements

### REQ-[ABBREV]-001: [User Benefit Title]

WHEN [trigger condition]
THE SYSTEM SHALL [expected behavior]

WHEN [edge case]
THE SYSTEM SHALL [error handling]

**Rationale:** [Why does the USER care?]

**Dependencies:** REQ-[ABBREV]-002 (if applicable)

---

### REQ-[ABBREV]-002: [Next Requirement]

WHEN [condition]
THE SYSTEM SHALL [behavior]

**Rationale:** [User benefit explanation]

---
```

**Rules:**
- NO status fields (status lives in executive.md)
- NO implementation details (belongs in design.md)
- NO “Updated YYYY-MM-DD” notes (git tracks history)
- IDs are IMMUTABLE once assigned
- Titles describe USER BENEFITS
- Rationale answers “Why does the USER care?”

### design.md

```markdown
# [Feature Name] - Technical Design

## Architecture Overview

[High-level description of how components interact]

## Data Models

[Structs, interfaces, schemas - language/framework agnostic where possible]

## API Contracts

[Endpoint specifications if applicable]

## Component Interactions

[Sequence diagrams, data flow descriptions]

## Error Handling Strategy

[How errors are detected, reported, recovered]

## Security Considerations

[Authentication, authorization, data validation]

## Performance Considerations

[Caching, optimization strategies]

## Per-Requirement Design

### REQ-[ABBREV]-001: [Title]
- Location: [file paths where this is/will be implemented]
- Approach: [technical approach and rationale]
- Trade-offs: [decisions made and why]
- Constraints: [invariants, performance bounds, compatibility rules]
```

**Rules:**
- Written BEFORE implementation, updated as implementation reveals new insights
- ALL content must trace to a REQ-* in requirements.md
- NO requirement definitions (belongs in requirements.md)
- NO status tracking (belongs in executive.md) -- design.md describes the
  technical approach, not whether it’s done.
  “Implemented in src/foo.rs, tests passing” is status.
  “Rate limiting uses a sliding window counter, stored in-process” is design.
- NO “Future Considerations” or content without corresponding requirements
- Must be self-contained (see “Self-Containment” and “Banned phrases” in
  SKILL.md)

### executive.md

```markdown
# [Feature Name] - Executive Summary

## Requirements Summary

[250 words max, user-focused: What problem does this solve? What can users do?]

## Technical Summary

[250 words max, architecture-focused: How is it built? Key technical decisions?]

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-[ABBREV]-001:** [Title] | ✅ Complete | Verified via [method] |
| **REQ-[ABBREV]-002:** [Title] | 🔄 In Progress | [Component] done, [other] pending |
| **REQ-[ABBREV]-003:** [Title] | ❌ Not Started | Planned for next iteration |

**Progress:** X of Y complete
```

**Rules:**
- 250 words max per summary
- ZERO code blocks (no triple backticks)
- Inline backticks for technical terms are fine (`config.yaml`, `src/auth.ts`)
- Include requirement titles in status table
- Use status symbols: ✅ 🔄 ⏭️ ❌ ⚠️ N/A

* * *

## Quality Checklists

Run these on every requirement before committing.

### User-Centricity Check

The purpose of this check is not formatting compliance -- it’s detecting hollow
requirements. A requirement that passes every structural rule but doesn’t trace
to a real user doing a real thing is busywork.
See “The Foundational Principle” in SKILL.md.

- [ ] Can you name a specific user (or persona) who benefits from this?
- [ ] Can you describe the journey that leads them to this requirement?
- [ ] Title describes USER BENEFIT, not system feature
- [ ] WHEN clause describes user action/context, not system internals
- [ ] SHALL clause describes observable user outcome
- [ ] Rationale answers “why does the user care?”
  with a concrete answer
- [ ] Non-technical user could understand the value

### Implementation-Creep Check

- [ ] No data structure field names (geohash, latitude, timestamp)
- [ ] No algorithm/technology names (Redis, JWT, HTTP endpoint)
- [ ] No “HOW” details (belongs in design.md)
- [ ] No code-like language or jargon
- [ ] No references to current system state or migration concerns

### Testability Check

- [ ] Observable behavior verifiable without reading code
- [ ] Specific criteria (numbers, states, messages)
- [ ] Clear success/failure conditions

### Self-Containment Check

- [ ] No time-dependent references (see “Banned phrases” under
  “Self-Containment” in SKILL.md)
- [ ] No comparative language requiring prior knowledge
- [ ] Document fully understandable standalone
- [ ] Both positive and negative cases explicitly stated

### Scope Check (design.md)

- [ ] Every section traces to a REQ-* in requirements.md
- [ ] No “Future Considerations” or “Phase 2” sections
- [ ] No speculative extensibility without corresponding requirements

* * *

## Common Operations

### Adding a New Requirement

1. Find highest existing ID for that abbreviation
2. Assign next sequential ID (never reuse)
3. Write EARS-formatted statement
4. Add user-focused rationale
5. Update executive.md with ❌ Not Started status

### Updating an Existing Requirement

1. Keep ID unchanged
2. Update EARS statements (git shows history)
3. Update design.md if implementation approach changes
4. Update executive.md status if needed

### Deprecating a Requirement

1. Do NOT delete from requirements.md
2. Add: **DEPRECATED:** Replaced by REQ-XX-###
3. Preserve original EARS statements
4. Add deprecation reason
5. Update executive.md status

* * *

## Red Flags (Rewrite Immediately)

- Requirement title ending in “-ing” (processing, caching, querying)
- “The system SHALL return/include/store/cache...” (implementation language)
- Technical acronyms without user context (WGS84, JWT, HTTP 429)
- Rationale mentioning database, cache, algorithm, or data structure
- “backwards compatible” or “migration” in requirements
- design.md content without a corresponding REQ-*

## Green Flags

- Title starts with user action verb (Discover, Show, Enable, View)
- WHEN clause starts with “When a user …” or “When exploring …”
- SHALL clause describes what user sees/experiences
- Rationale uses words like: curiosity, discover, explore, understand, trust
