# Make WorkDisposition invariants structural and remove CPU-sensitive exhaustive tests

## Observed journey

- Under high CPU, Vitest times out after 5 seconds in `src/components/workDisposition.test.ts > structural invariants > hidden bars carry safe defaults`.
- The requested fix is to remove the structural cause of the flake, not increase `testTimeout` or otherwise make correctness depend on wall-clock timing.

## Verified findings

- `matrix()` in `ui/src/components/workDisposition.test.ts` emits 38,400 cases per invocation. Five invariant tests independently traverse it, causing 192,000 derivations and many more individual Vitest assertions.
- The null-`prStatus` case is yielded inside the `found × display_state × check_state × refresh` loops even though none of those dimensions affect it. Each outer null-status input is therefore repeated 150 times.
- `found: false` statuses are also repeated across `display_state` and `check_state`, fields that are not placed on that status shape. The matrix additionally manufactures semantically inconsistent/unreachable shapes such as `found: true` with no display state.
- The timed-out test performs up to four framework assertions for every hidden result, so assertion/reporting overhead grows with the redundant Cartesian product and becomes sensitive to CPU contention.
- `deriveWorkDisposition` is a synchronous pure function with no timers, I/O, randomness, React, or DOM dependency. The observed failure is test workload exhaustion, not timing behavior in production code.
- `WorkDisposition` is currently a flat interface. It permits invalid values such as a hidden bar with a resolve primary, a resolve primary with `resolve: null`, or a secondary resolve beside a non-address primary. Runtime matrix tests are compensating for states the type allows.
- This contradicts the code and REQ-WAB-003 rationale that invalid primary/resolve combinations should be structurally unrepresentable. The Allium model also defines `WorkDisposition` as variants rather than independent booleans.
- `WorkActions.tsx` is the only production consumer. It already returns early for hidden dispositions, but its `primaryClass` helper is currently declared before that narrowing point.

## Failure model

A large, redundant Cartesian-product test tries to prove output-shape invariants dynamically because the output type does not encode them. Under contention, Vitest assertion overhead crosses the global per-test timeout. Increasing the timeout would retain both defects: CPU-sensitive coverage and representable invalid disposition states.

## Proposed scope

### 1. Encode the output contract as a discriminated union

Refactor `WorkDisposition` in `ui/src/components/workDisposition.ts` so TypeScript rejects invalid combinations at construction time. Preserve the rendered behavior and existing public semantics while making at least these relationships structural:

- hidden disposition ⇒ `primary: 'none'`, no resolve/secondary, no terminal verbs, no note;
- `primary: 'resolve'` ⇔ a primary resolve verb is present;
- a secondary resolve is possible only beside an `address_feedback` primary and is restricted to GitHub link-out verbs;
- a visible disposition without Abandon is the continued disposition;
- a `gh_unavailable` note belongs only to the Clean-up disposition.

Use a discriminant aligned with the semantic disposition variants where that keeps narrowing and construction clear. Do not add parallel state that duplicates `primary`; either derive presentation fields from the variant or constrain them within the union so there remains one authoritative representation.

Update `hidden`, `resolveVerb`, `reviewPrimary`, `finish`, and direct return sites so every constructor has a precise return type and the compiler checks each invariant. Update `WorkActions.tsx` to narrow hidden/visible and disposition variants before reading variant-specific fields; move helpers below the narrowing point as needed.

### 2. Replace brute-force invariant assertions with bounded coverage

Refactor the structural-invariant section of `ui/src/components/workDisposition.test.ts`:

- remove the 38,400-case-per-test Cartesian product and the meaningless `count > 1000` assertion;
- use compile-time/type-level assertions where the discriminated union now makes invalid output shapes impossible;
- retain small runtime decision-table coverage for the first-match ordering and meaningful equivalence classes from REQ-WAB-001/003/004/005/009 (hidden, continued, checking, stuck PR states, idle open/draft branches, terminal PR states, gh unavailable, and no-PR work-change states);
- if a generated matrix remains useful for totality, generate only reachable canonical status shapes, deduplicate irrelevant dimensions before iteration, derive each case once, and apply all remaining runtime predicates in that single pass.

Do not solve the flake with a larger timeout, sleeps, retries, reduced Vitest concurrency, or an arbitrary random sample.

### 3. Keep normative artifacts accurate

The requirements and Allium behavior already require a total derivation and structural single-primary contract. No behavior change is intended. Update only current-reality/verification documentation if the implementation anchors or verification description materially change; do not rewrite normative behavior to match implementation convenience.

## Validation

- Run the focused test repeatedly: `cd ui && pnpm test -- src/components/workDisposition.test.ts` (or the repository-approved equivalent through `./dev.py`).
- Run UI typecheck/lint so intentionally invalid disposition fixtures would fail compilation and all `WorkActions.tsx` narrowing is checked.
- Run `./dev.py check` before completion.
- Confirm the focused test remains comfortably below the default 5-second timeout under CPU contention without any timeout override.
- Confirm existing Work Actions component tests still cover rendered verbs and single-primary presentation.

## Acceptance criteria

- The reported `hidden bars carry safe defaults` timeout no longer depends on host CPU timing.
- Invalid hidden/resolve/secondary/continued/gh-unavailable output combinations listed above are rejected by TypeScript rather than merely caught by an exhaustive runtime loop.
- Runtime tests cover semantic decision boundaries without redundant or unreachable Cartesian-product inputs.
- No user-visible Work Actions behavior changes.
- No timeout, retry, sleep, or test-runner concurrency workaround is introduced.

## Risks and non-goals

- Risk: a broad type refactor can create noisy renderer changes. Keep the union local to `workDisposition.ts` and its sole production consumer, and prefer exhaustive narrowing over casts or non-null assertions.
- Risk: removing the matrix could accidentally remove decision coverage. Preserve explicit tests for every REQ-WAB-004 row and precedence boundary; remove only redundant shape-invariant checks now enforced by types.
- Non-goal: redesigning Work Actions UI, PR association, refresh behavior, work-change semantics, or lifecycle endpoints.
- Non-goal: globally tuning Vitest timeouts or performance.
