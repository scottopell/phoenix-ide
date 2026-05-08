# spEARS Spec Linter

Scan spec documents for quality violations and fix them.
Unlike validation (which audits status accuracy against the codebase), linting
enforces document quality rules and rewrites violations in place.

## When to Use

- Migrating existing specs to follow spEARS best practices
- Periodic cleanup of specs that have drifted
- After a large implementation push where agents may have polluted specs
- “Lint all my specs” or “clean up my design docs”

## Input Handling

Accept either:
- A specific spec path (`specs/my-feature`) -- lint that single spec
- A specific document (`specs/my-feature/design.md`) -- lint that file only
- No path or “all” -- lint all specs in the project

## Lint Rules

Each rule maps to a guardrail in SKILL.md.
For every violation found, rewrite the offending content -- don’t just flag it.

### Rule 1: Self-Containment Violations

**Applies to:** All three documents (design.md most commonly)

**Detect:** Scan for banned phrases and comparative language:
- “as before” / “as currently implemented” / “previously”
- “maintain existing behavior” / “continue to work as expected”
- “unlike the old approach” / “the improved version”
- “same as [other feature]” / “following the established pattern”
- “standard validation rules” / “default timeout values”
- “backwards compatible” / “migration from current”
- “better than” / “faster than” / “more secure than” (without a stated baseline)

Also detect contextual self-containment failures -- statements that only make
sense if you were part of the conversation that produced them:
- “This avoids the [problem] we had” -- describe the constraint, not the history
- “This design doesn’t violate X” -- describe what the design DOES, not what it
  avoids. Negative framing implies a prior discussion about X.
- “We chose this because of the incident with Y” -- state the constraint Y
  revealed, not the incident itself

**Fix:** Replace with concrete, self-contained descriptions.
The rewritten content should describe the actual behavior, constraint, or
property -- not reference something the reader would need external context to
understand.

**Example:**

Before: “This approach avoids the thread safety issues we encountered.”

After: “The worker pool processes items concurrently across N threads.
Shared state is confined to the job queue, which uses a mutex-guarded channel.
Workers hold no references to each other’s state, eliminating data races by
construction.”

Also detect **phasing language in design.md** -- a distinct and common pattern
where scheduling/prioritization decisions leak into the technical design.
See “Design.md phasing language” in SKILL.md for the full rationale.

Scan design.md for:
- “start with” / “initially” / “as a first pass” / “for now”
- “deferred” / “deferred to” / “future work” / “later”
- “prioritize” / “prioritize getting X working early”
- “once X is stable” / “once X is done” / “after X is complete”
- “incrementally” (when describing delivery order, not system behavior)

**Fix:** Separate the design from the schedule.
If the design has two operating modes (full and reduced-fidelity), describe both
as architectural properties.
Move the phasing decision ("we’re building the simple one first") to
executive.md.

**Example:**

Before (design.md): “Start with SQLite-only data and add LevelDB parsing
incrementally.”

After (design.md): “The data layer reads from both SQLite and LevelDB. When
LevelDB data is unavailable, the system operates in reduced-fidelity mode.”

After (executive.md): “MVP targets SQLite-only.
LevelDB parsing planned for next iteration.”

### Rule 2: Document Boundary Violations

**Detect in requirements.md:**
- Status indicators (✅, 🔄, ❌, “Status:”, “Complete”, “In Progress”)
- Implementation details ("uses Redis", “stored in table X”, “via HTTP
  endpoint”)
- Progress/milestone language ("Phase 1", “MVP”, “next iteration”)
- “Future ideas” or roadmap content
- Data structure field names used as if the reader should know them
- References to current system state ("backwards compatible", “migration”)

**Detect in design.md:**
- Status tracking ("implemented", “tests passing”, “done”, “complete”)
- Requirement definitions (WHEN/SHALL statements that belong in requirements.md)
- Content that doesn’t trace to any REQ-* in the corresponding requirements.md
- “Future Considerations”, “Phase 2”, “Planned Enhancements” sections
- Speculative extensibility without a corresponding requirement

**Detect in executive.md:**
- Code blocks (triple backticks) -- inline backticks are fine
- Summaries exceeding 250 words
- Missing requirement titles in status table (just IDs without titles)

**Fix:**
- Move misplaced content to the correct document
- Remove content that doesn’t belong anywhere (e.g., status in requirements.md
  is simply deleted -- executive.md is the source of truth for status)
- For orphaned design.md content (no corresponding REQ-*), remove it and note
  what was removed in the lint report so the user can decide whether to create a
  requirement for it or discard it

### Rule 3: Hollow Requirement Detection

This is the most important lint rule.
A structurally perfect requirement that doesn’t trace to a real user need is
busywork. See “The Foundational Principle” in SKILL.md.

**Detect in requirements.md:**
- Requirements that cannot be traced to a user persona or journey -- if you
  can’t answer “who benefits and during what activity?”, the requirement is
  hollow regardless of formatting
- Titles that describe system features instead of user benefits (look for:
  “-ing” suffixes, technology names, data structure names)
- Vague EARS statements ("be fast", “handle errors gracefully”, “good UX”)
- Rationale that explains technical interest ("enables efficient querying")
  instead of user value ("users can find what they need without waiting")
- Implementation leak in WHEN/SHALL clauses (technology names, field names,
  algorithm names)
- Requirements that read like they exist because “the system should probably do
  this” rather than because a user has a concrete need

**Fix:** Rewrite titles to focus on user benefit.
Rewrite EARS statements to be specific and testable.
Rewrite rationale to answer “why does the user care?”

Flag but don’t auto-fix requirements where the intent is unclear -- ask the user
what the requirement actually means before rewriting.
Hollow requirements often can’t be fixed by rewording alone; they may need to be
reconsidered through discovery (see `references/discover.md`).

### Rule 4: Structural Issues

**Detect:**
- Requirement IDs that don’t follow REQ-[ABBREV]-### format
- Duplicate requirement IDs
- Missing rationale on requirements
- design.md “Per-Requirement Design” sections referencing REQ-* IDs that don’t
  exist in requirements.md
- executive.md referencing requirements not in requirements.md
- Spec directories not using kebab-case

**Fix:** Fix formatting issues.
Flag ID conflicts and missing requirements for user decision.

## Workflow

### 1. Scan

Read all target spec documents.
Build a map of:
- All REQ-* IDs defined in requirements.md files
- All REQ-* IDs referenced in design.md and executive.md files
- Cross-spec references

### 2. Lint

Apply all rules. For each violation, determine whether it can be auto-fixed or
needs user input.

**Auto-fix:** Self-containment rewrites, document boundary moves, structural
formatting, status removal from requirements.md.

**Ask first:** Unclear requirement intent, orphaned design content that might
need a new requirement, requirement quality rewrites where the original intent
is ambiguous.

### 3. Fix

Apply all auto-fixes.
Present user-decision items as a batch.

### 4. Report

```markdown
# Lint Report: [scope]

## Summary
- Files scanned: X
- Violations found: Y
- Auto-fixed: Z
- Needs user decision: W

## Auto-Fixed

### [spec-name]/design.md
- **Line 45**: Self-containment violation. Rewrote "avoids previous issues
  with thread safety" to describe actual concurrency constraints.
- **Line 82**: Status removed. "Implemented and tested" moved to
  executive.md or removed if already tracked there.

### [spec-name]/requirements.md
- **REQ-XX-003**: Title rewritten from "JWT Token Validation" to
  "Secure User Sessions"

## Needs Decision

### [spec-name]/design.md
- **Lines 120-145**: "Future Considerations" section has no corresponding
  requirements. Remove entirely, or create requirements for these capabilities?
  Content: [brief summary of what's there]

### [spec-name]/requirements.md
- **REQ-XX-005**: "THE SYSTEM SHALL handle edge cases properly" -- too vague
  to rewrite without understanding intent. What specific edge cases and
  behaviors are meant?
```

## Bulk Operations

When linting all specs in a project:

1. Scan all `specs/*/` directories
2. Process each spec independently
3. Check for cross-spec issues (duplicate REQ abbreviations, orphaned
   cross-references)
4. Aggregate the report with per-spec sections
5. Present auto-fixes first, then batch all user-decision items at the end
