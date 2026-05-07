# spEARS Implementation Workflow

This workflow guides implementing requirements from completed specs.
The correct order is always: Requirements -> Design -> Tests -> Implementation.

## Core Principles

1. **YAGNI**: Only implement what’s in requirements.md.
   No extras.
2. **Follow Design**: design.md is the implementation guide.
   Don’t invent.
3. **Verify Tests**: Requirements aren’t complete until tests pass.
4. **Track Status**: executive.md must reflect current reality.

## Input Handling

Accept either:

- **Requirement IDs**: `REQ-RL-001`, `REQ-RL-002` -- implement specific
  requirements
- **Spec paths**: `specs/rate-limiting` -- implement all non-complete
  requirements

When given requirement IDs, locate the containing spec directory first.

## Pre-Implementation Checklist

Before writing any code:

1. **Read all three spec documents**
   - requirements.md: understand WHAT to build
   - design.md: understand HOW to build it
   - executive.md: understand current status

2. **Identify target requirements**
   - If given IDs: validate they exist in requirements.md
   - If given spec path: find all requirements not marked ✅ in executive.md

3. **Analyze dependencies**
   - Check if requirements depend on each other
   - Sort into implementation order

4. **Verify design coverage**
   - For each requirement, confirm design.md has implementation guidance
   - If design lacks detail: STOP and ask the user

## Per-Requirement Workflow

For each requirement, in dependency order:

### 1. Update Status

Edit executive.md: ❌ -> 🔄

### 2. Follow design.md

Read the implementation guidance.
The design specifies architecture approach, data models, API contracts, and
error handling.

If design.md doesn’t specify something you need to decide: do NOT guess.
Ask the user for direction.

### 3. Write Implementation

- Add requirement comments: `// REQ-XX-###: Brief description`
- Follow existing code patterns in the codebase
- Implement ONLY what the requirement specifies

### 4. Write/Update Tests

- Add `@requirement REQ-XX-###` or `// REQ-XX-###` tags
- Tests should verify the EARS “SHALL” clauses
- Cover success paths AND error conditions from requirements

### 5. Run Tests

Execute the project’s test suite.
Adapt to whatever test runner the project uses.

### 6. Update Status

Only if tests pass: edit executive.md: 🔄 -> ✅

If tests fail: fix the implementation, re-run, only mark complete when passing.

## YAGNI Enforcement

Do NOT implement:

- Features not in requirements.md (even if they seem useful)
- “Future-proofing” abstractions not required by current requirements
- Extra configuration options beyond what requirements specify
- Additional error handling beyond what requirements specify

**Stop signals** -- if you catch yourself thinking any of these, check whether a
requirement actually asks for it:

- “While I’m here, I’ll also …”
- “It would be nice to also …”
- “For extensibility, let me add …”
- “In case we need it later …”

## Design Gap Protocol

When design.md lacks detail for a decision:

1. Identify the specific gap
2. Formulate a clear question with options
3. Ask the user for direction
4. Proceed with their guidance
5. Note the decision in code comments

## Error Recovery

If implementation reveals a design flaw:

1. STOP -- don’t work around it
2. Report: “design.md specifies X, but this conflicts with Y”
3. Ask the user how to proceed
4. Typical options: update design.md then continue, or pause and revisit

## Completion Criteria

A requirement is ✅ Complete when:

- Implementation matches design.md
- Code has requirement comments (`// REQ-XX-###`)
- Tests exist with requirement tags
- All tests pass
- executive.md updated to ✅
- No un-spec’d features were added

## Progress Reporting

Report at requirement-level granularity:

```text
Starting REQ-RL-001: Request Rate Visibility
  Updated executive.md: ❌ -> 🔄
  Following design: sliding window counter in middleware
  Implementation: src/middleware/rate_limit.rs
  Tests: tests/rate_limit_test.rs
  Tests passed
  Updated executive.md: 🔄 -> ✅
Completed REQ-RL-001
```

## Session Summary Format

At end of session:

```text
## Implementation Summary

### Completed
- REQ-XX-001: [Title] - [brief note]

### In Progress (if interrupted)
- REQ-XX-003: [Title] - [current state, what's left]

### Blocked (if any)
- REQ-XX-004: [Title] - [reason, what's needed]

### Files Modified
- src/foo.rs (REQ-XX-001, REQ-XX-002)
- tests/foo_test.rs (REQ-XX-001, REQ-XX-002)
- specs/feature/executive.md (status updates)

### Test Results
All tests passing
```
