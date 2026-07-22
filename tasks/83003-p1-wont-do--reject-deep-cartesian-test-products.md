# Reject deep Cartesian products in TypeScript tests

## Disposition

Wont do. The flake mechanism was a giant synchronous test crossing Vitest's fixed wall-clock timeout under CPU contention, not loop nesting itself. Nesting depth is an unreliable workload proxy: small four-dimensional products can be cheap, large lower-dimensional products can be expensive, and helper reuse can hide the same cost. A syntax rule would encode a misleading threshold and create both false positives and false negatives. The structural type fix and focused decision-boundary tests address the concrete defect; future giant-test cases should be handled through profiling and review rather than this lint.

## Observed journey

A structural-invariant Vitest test generated a nine-dimensional Cartesian product, repeated the same derivation across five tests, and intermittently exceeded Vitest's five-second timeout under CPU contention. The product code was synchronous and deterministic; the flake came from unbounded/redundant test workload and assertion overhead.

## Verified findings

- Phoenix's anti-flake prevention already runs syntax-specific ast-grep rules in the `ast-grep` check lane over `ui/src/` and `crates/`.
- Existing TypeScript and TSX anti-flake rules are paired by parser language and reject precise timing smells rather than broad classes of test code.
- The pre-fix pattern contains nine directly nested `for...of` loops. An ast-grep prototype matching four directly nested `for...of` loops detects it.
- The prototype reports zero findings across the current `ui/src/**/*.test.ts` and `ui/src/**/*.test.tsx` tree.
- Phoenix has legitimate generated-history/state-machine tests, so banning generators, loops, invariant tests, or all Cartesian products would create false positives.
- A two- or three-dimensional table can remain reasonable and bounded; four independent dimensions already multiply rapidly and should require explicit restructuring/review.
- ast-grep's `files:` filter correctly limits the prototype to test files and ignores the same syntax in production TypeScript.
- The repository currently has no checked-in positive/negative fixture harness for ast-grep rules. This rule's threshold and file scoping are policy, so they need executable calibration rather than relying only on a clean-tree scan.

## Failure model

Deep nested products hide multiplicative test cost in visually small code. Repeating the generator or placing multiple assertions inside the product makes runtime proportional to the product of every dimension and scheduler-sensitive, while a normal code review sees only a handful of loops. A wall-clock timeout then becomes an unreliable proxy for a workload budget.

## Proposed scope

### Add paired structural rules

Add TypeScript and TSX ast-grep rules that reject four or more directly nested `for...of` loops only in `**/*.test.ts` and `**/*.test.tsx` files.

The diagnostic should explain that deep Cartesian products make test cost multiplicative and CPU-sensitive, and recommend one or more of:

- encode shape invariants in discriminated unions/type-level assertions;
- use focused equivalence classes or `test.each` decision tables;
- deduplicate irrelevant dimensions and derive each canonical case once;
- split genuinely independent dimensions into focused tests.

Do not recommend increasing test timeouts, random sampling, or runner-concurrency changes.

### Add executable rule calibration

Add a small automated test around the rule behavior, following the repository's existing Python/dev-check conventions or a comparably lightweight harness. It must prove:

- four nested loops in a `.test.ts` file fail;
- four nested loops in a `.test.tsx` file fail;
- three nested loops in test files pass;
- four nested loops in production `.ts`/`.tsx` files pass;
- statements between nesting levels do not evade detection;
- the diagnostic identifies the structural workload problem.

Keep fixtures synthetic and tiny. Do not copy the full historical test.

### Preserve check integration

Use the existing automatic `ast-grep-rules/*.yml` discovery and ASTGREP path categorization. Change `dev.py` only if executable fixture calibration cannot be integrated without it; do not add another full source-tree parse to the normal check lane.

## Validation

- Run the new positive/negative rule tests.
- Run the new paired rules against the current `ui/src` tree and confirm zero findings.
- Temporarily verify the synthetic pre-fix shape is rejected by both parsers.
- Run `./dev.py check`.

## Acceptance criteria

- New four-level nested `for...of` products cannot be introduced in TypeScript or TSX test files without an ast-grep failure.
- Three-level products and production code are not flagged.
- The historical bug shape is caught without a timeout, retry, or runtime benchmark.
- Rule behavior and threshold are covered by committed automated tests.
- Existing source passes without allowlist entries.

## Risks and non-goals

- This is a tripwire, not a general static complexity analyzer. It does not attempt to multiply array cardinalities or detect products expressed through `flatMap`, recursion, helper calls, or property-testing libraries.
- It does not ban table-driven, generated-history, state-machine, or property tests.
- It does not impose a global wall-clock duration threshold.
- Avoid exemptions initially; if a legitimate four-dimensional exhaustive test appears, prefer an explicit bounded helper/design and revisit the rule with evidence rather than adding a broad allowlist.
