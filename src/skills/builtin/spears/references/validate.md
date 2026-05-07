# spEARS Spec Validation

Validation audits spec directories for correctness, with special focus on
executive.md accuracy and design.md scope integrity.

## Input Handling

Accept either:
- A specific spec path (e.g., `specs/my-feature`) -- validate that single spec
- No path or “all” -- validate all specs in the project

For multiple specs, validate each one and aggregate findings.

## Validation Categories

### 1. Structural Linting

**requirements.md:**
- [ ] Uses EARS format (WHEN/WHILE/WHERE/IF + THE SYSTEM SHALL)
- [ ] Has immutable IDs: REQ-[ABBREV]-###
- [ ] NO status fields
- [ ] NO implementation details
- [ ] Titles describe USER BENEFITS
- [ ] Rationale answers “Why does the USER care?”
- [ ] No references to current system state or migration concerns

**design.md:**
- [ ] All content traces to a REQ-* in requirements.md
- [ ] NO requirement definitions
- [ ] NO status tracking
- [ ] NO “Future Considerations” or “Phase 2” sections
- [ ] Self-contained (no “as before”, “improved”, “previously” without
  specifics)
- [ ] No comparative language referencing unstated baselines

**executive.md:**
- [ ] ZERO code blocks (triple backticks)
- [ ] 250 words max per summary section
- [ ] Status table includes requirement titles
- [ ] Uses valid status symbols: ✅ 🔄 ⏭️ ❌ ⚠️ N/A
- [ ] Progress count is accurate

### 2. Executive.md Accuracy (Cross-Reference)

Executive.md is the ONLY document that must reflect current reality.
Verify:

**Status Claims:**
- For ✅ Complete: check that implementation files exist, look for test files
  with requirement tags, grep codebase for requirement ID references
- For 🔄 In Progress: verify partial implementation exists
- For ❌ Not Started: confirm no implementation references exist (flag if found)

**File Path Claims:**
- If executive.md or design.md mentions specific file paths, verify they exist

**Summary Accuracy:**
- Requirements Summary should match what’s in requirements.md
- Technical Summary should match what’s in design.md

### 3. Design.md Scope Check

Flag when design.md references features or capabilities with NO corresponding
requirement in requirements.md.

**How to detect:**
1. Extract all requirement IDs from requirements.md
2. Scan design.md for:
   - Sections like “Future Considerations”, “Phase 2”, “Planned Enhancements”
   - References to capabilities not tied to a REQ-* ID
   - Language like “we could later add”, “future versions might”

**Important distinction:**
- Unimplemented requirements (in requirements.md with ❌ status) are FINE
- Speculative features not defined in requirements.md are NOT fine

### 4. Self-Containment Audit

Scan all three documents for banned phrases and comparative language:
- “as before” / “as currently implemented” / “previously”
- “maintain existing behavior” / “unlike the old approach”
- “the improved version” / “better than before”
- “standard validation rules” / “default timeout values”
- “backwards compatible” / “migration from current”

Flag each occurrence with the specific line and a suggested rewrite.

## Output Format

```markdown
# Spec Validation Report: [spec-name]

## Summary
- Overall: [PASS | WARN | FAIL]
- Structural Issues: X
- Accuracy Issues: X
- Scope Issues: X
- Self-Containment Issues: X

## Structural Linting

### requirements.md
[Findings or "No issues"]

### design.md
[Findings or "No issues"]

### executive.md
[Findings or "No issues"]

## Executive.md Accuracy

### Status Verification
| Requirement | Claimed Status | Verified | Notes |
|-------------|----------------|----------|-------|
| REQ-XX-001 | ✅ | ✅ | Found in src/foo.rs, test in tests/foo.rs |
| REQ-XX-002 | ✅ | ⚠️ | No test file references this requirement |

### File Path Verification
[List of paths checked and whether they exist]

## Scope Check
[Orphaned content in design.md, or "None found"]

## Self-Containment Check
[Banned phrases found with line numbers and suggested rewrites]

## Recommendations
[Actionable items to fix issues]
```

## Severity Levels

- **FAIL**: executive.md claims ✅ Complete but no implementation evidence
- **WARN**: Missing test references, orphaned design content, self-containment
  violations, minor structural issues
- **PASS**: All checks pass with no or only informational findings

## Verification Commands

```bash
# All references to a requirement
rg "REQ-XX-###"

# All requirements in a feature
rg "^### REQ-" specs/feature-name/requirements.md

# All requirement comments in code
rg "// REQ-" src/

# Check for banned phrases in specs
rg -i "as before|as currently|previously|maintain existing|backwards compatible" specs/
```
