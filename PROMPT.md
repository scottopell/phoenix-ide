# MVP Scope Review

**CRITICAL: Do NOT make any tool calls until you have read ALL spec files.**

You are reviewing PhoenixIDE's spEARS specifications to ensure they define a
coherent, well-scoped MVP. Your goal is to identify gaps, overlaps,
inconsistencies, and scope creep before implementation begins.

## Review Principles

1. **KISS**: Is this the simplest design that could work?
2. **Coherence**: Do specs reference each other correctly? No orphaned concepts?
3. **Completeness**: Can we build a working system from these specs alone?
4. **No Gold Plating**: Are we specifying only what's needed for MVP?

## Step 1: Read All Specs

Read every file in `specs/*/` before analyzing:

```
specs/
├── bedrock/     # Core conversation state machine
├── bash/        # Bash tool
├── patch/       # Patch tool  
├── llm/         # LLM provider abstraction
└── api/         # HTTP API
```

For each spec, read:
- requirements.md (WHAT)
- design.md (HOW)
- executive.md (STATUS)

## Step 2: Cross-Spec Analysis

### Dependency Graph

Map how specs depend on each other:
- Which specs reference requirements from other specs?
- Are all references valid (REQ-XXX-### exists)?
- Are there circular dependencies?

### Interface Boundaries

Verify clean interfaces between specs:
- Does bedrock define events that llm needs to produce?
- Does api define endpoints that match bedrock's state model?
- Do tool specs (bash, patch) define schemas that bedrock can execute?

### Missing Links

Identify gaps:
- Concepts mentioned but never defined
- Requirements that reference undefined behavior
- State machine events with no producer
- API endpoints with no corresponding state machine handling

## Step 3: KISS Analysis

### Unnecessary Complexity

Flag over-engineering:
- Features that could be simpler
- Abstractions that don't pay for themselves
- Configuration options that could be hardcoded for MVP
- Error handling that exceeds what's needed

### Scope Creep Indicators

Red flags:
- "Future considerations" or "Phase 2" language
- Requirements that aren't essential for basic functionality
- Nice-to-have features disguised as must-haves
- Extensibility hooks for unspecified use cases

### Missing Simplifications

Could we simplify by:
- Deferring a feature entirely?
- Hardcoding instead of configuring?
- Reducing state machine states?
- Combining similar requirements?

## Step 4: Implementation Readiness

### Can We Build This?

For each spec, ask:
- Is there enough detail to implement without guessing?
- Are edge cases specified or at least acknowledged?
- Are error conditions defined?
- Is the happy path clear?

### Technical Feasibility

- Any requirements that conflict with each other?
- Any requirements that are technically impossible or very hard?
- Dependencies on external systems that aren't specified?

### Test Strategy

- Can each requirement be tested as specified?
- Are acceptance criteria clear enough to write tests?

## Output Format

```markdown
# MVP Scope Review: PhoenixIDE

## Executive Summary
[2-3 sentences: Is this MVP well-scoped? Major concerns?]

## Spec Inventory
| Spec | Requirements | Status | Notes |
|------|--------------|--------|-------|
| bedrock | 12 | Ready | Core state machine |
| bash | 6 | Ready | ... |
| ... | ... | ... | ... |

## Cross-Spec Analysis

### Dependency Graph
[Describe how specs relate]

### Interface Issues
[List mismatches between specs]

### Missing Links
[Concepts referenced but undefined]

## KISS Violations

### Unnecessary Complexity
| Spec | Issue | Recommendation |
|------|-------|----------------|
| ... | ... | ... |

### Scope Creep
| Spec | Requirement | Issue |
|------|-------------|-------|
| ... | REQ-XXX-### | Should defer to post-MVP |

### Simplification Opportunities  
[List ways to reduce scope]

## Implementation Readiness

### Ready to Implement
[Specs/requirements that are well-defined]

### Needs Clarification
| Spec | Requirement | Question |
|------|-------------|----------|
| ... | REQ-XXX-### | How does X interact with Y? |

### Technical Concerns
[Any feasibility issues]

## Recommendations

### Must Fix Before Implementation
1. [Critical issues]

### Should Consider
1. [Important but not blocking]

### Nice to Have
1. [Minor improvements]

## Conclusion
[Final assessment: Ready for implementation? What needs to happen first?]
```

## Evaluation Criteria

**PASS**: Specs are coherent, complete enough to implement, appropriately scoped
**NEEDS WORK**: Some gaps or issues but fundamentally sound
**FAIL**: Major gaps, conflicts, or scope problems that must be resolved
