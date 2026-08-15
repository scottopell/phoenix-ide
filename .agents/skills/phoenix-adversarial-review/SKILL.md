---
name: phoenix-adversarial-review
description: Adversarially review an agent-authored diff, PR, commit, or implementation by trying to falsify its correctness before merge. Use when asked for a rigorous code review, independent challenge, exact-HEAD review, self-review blind-spot check, or comparison with Codex findings. Especially useful for Phoenix changes involving invariants, persistence, wire contracts, concurrency, recovery, security boundaries, tests, or unnecessary complexity.
argument-hint: <diff-or-pr-scope>
---

# Phoenix Adversarial Review

Try to break the change, not explain it. A useful review produces a small set of source-grounded defects—or says that no candidate survived falsification.

Repository requirements, Allium, ADRs, and task/PR intent are authorities. Prior reviews, production records, and Codex findings are evidence, never requirements.

## 1. Freeze the review target

Before reading conclusions from any prior review:

1. Record the repository, base commit, reviewed head commit, and working-tree cleanliness.
2. Refuse an **exact-HEAD** claim unless the local tree SHA and external review `commit_id` are identical. A branch name, green CI run, request timestamp, or later summary does not prove identity.
3. If the tree is dirty, either review the dirty snapshot explicitly or stop and request a committed target. Never silently mix committed and uncommitted code.
4. Fetch the complete diff and changed-file list. Detect truncation, generated files, renames, migrations, and changes outside the apparent feature directory.

When comparing reviews, use the same frozen tree. If SHAs differ, label the evidence **unpaired** and do not infer reviewer blind spots from it.

## 2. Build the authority and blast-radius map

Read the smallest governing set before judging the patch:

- applicable `requirements.md` and `.allium` files;
- relevant ADRs and executive status;
- task/PR intent and explicit non-goals;
- nearby tests and the pre-change implementation at each changed seam.

Then state privately:

- the user-visible promise;
- the invariants the change relies on;
- producers, durable owners, and consumers of changed data;
- boundaries crossed: UI, API/SSE, runtime, persistence, provider/tool, process, host, or Git;
- what can outlive the request, process, worktree, or deployment.

Do not review only the edited lines. Trace one layer upstream and downstream of every changed contract.

## 3. Run adversarial probes

Select probes from [references/probes.md](references/probes.md) based on the blast-radius map. These are hypothesis generators, not a checklist quota.

Always perform these gap-informed moves when applicable:

- **Producer/consumer literal check:** compare discriminants, marker strings, field names, enum cases, encodings, and defaults at the exact write and read sites. Do not infer parity from shared terminology.
- **Incarnation check:** follow identity creation through restart, respawn, retry, rekey, migration, and reuse. An identity assigned correctly at construction may become stale when the resource is replaced in place.
- **Recovery without memory check:** erase process-local registries and caches mentally. Verify durable identity can reconstruct or safely reject surviving external resources.
- **Commit-before-effect check:** at every async boundary, cancellation path, or error return, ask which state/effect has already happened and whether its durable owner was established first.
- **One-authority check:** look for two representations, selectors, caches, timestamps, or status fields claiming the same fact.
- **Error-path data check:** follow observations and state changes through `Err`, timeout, stale, duplicate, partial-success, and cleanup paths—not only returned success values.
- **Test oracle check:** mutate the behavior mentally. Would the test fail for the right reason, or only prove that a mock was called / an error occurred?

For timer, retry, wake, cancellation, or concurrent code, trace causal completion and ownership. A sleep or timeout is not synchronization.

## 4. Falsify every candidate

A candidate becomes a finding only if all of these are present:

1. **Anchor:** precise file/symbol or changed contract.
2. **Trigger:** concrete reachable preconditions.
3. **Mechanism:** the code path from trigger to violated postcondition.
4. **Impact:** observable wrong behavior or broken invariant.
5. **Counterevidence check:** existing guard, type, test, caller precondition, or spec does not already disprove it.
6. **Scope check:** the defect exists in the frozen reviewed tree and was introduced or exposed by the change.

Reproduce or write the smallest falsifying test when practical. Otherwise state what evidence would verify the claim. Downgrade unsupported defects to **questions**; omit speculative style preferences entirely.

Severity reflects impact and reachability, not reviewer confidence:

- **P0:** immediate catastrophic loss, compromise, or system-wide outage.
- **P1:** reachable data loss, security failure, authority violation, or core workflow break.
- **P2:** reachable incorrect behavior with bounded impact or a material operational failure.
- **P3:** low-impact defect; do not report mere polish.

Report confidence separately as high, medium, or low. Findings below medium confidence require reproduction evidence or should remain questions.

## 5. Compare local review with Codex only after independent review

Do not read Codex findings before the independent pass; they anchor the search and hide genuine local blind spots.

For each same-HEAD pair, normalize findings by semantic defect rather than wording or line number and classify:

- **overlap:** both found the same violated postcondition;
- **local-only:** local finding not raised by Codex;
- **Codex-only:** Codex finding missed locally;
- **disputed:** concrete counterevidence defeats one review;
- **unpaired:** target identity is not proven equal.

For every Codex-only valid finding, name the missing **review move**, not just the defect category—for example, “compare emitted and parsed marker literals” or “trace generation rotation on respawn.” Add that move to the current review and use it as held-out validation later. Do not claim that accepted comments are correct solely because they were fixed, or that rejected comments are false solely because they were closed.

See [references/evidence-method.md](references/evidence-method.md) for the evidence and confidentiality rules behind this workflow.

## 6. Output findings, not a tour

Return findings first, ordered by severity and then confidence:

```markdown
## Findings

### [P1] Short imperative title — `path:line` or `Type::symbol`
**Confidence:** High
**Trigger:** Concrete preconditions.
**Mechanism:** Exact path from changed code to failure.
**Impact:** Violated invariant or user-visible result.
**Evidence:** Test, command, spec clause, or source comparison.
**Verify:** Smallest reproduction or regression test.
```

Then include, only when useful:

```markdown
## Questions
- A decision-blocking ambiguity with its competing interpretations.

## Review coverage
- Frozen target: `<base>..<head>`; clean/dirty; exact-HEAD status.
- Authorities and boundaries inspected.
- Tests/reproductions run and material limitations.

## Review delta
- overlap: N; local-only: N; Codex-only: N; disputed: N; unpaired: N
- Missing review moves learned from valid Codex-only findings.
```

If nothing survives falsification, say **“No actionable findings.”** Still report the frozen target, inspected boundaries, tests run, and limitations. Never manufacture a finding to make the review look productive.

## Anti-patterns

- Summarizing the PR instead of challenging it.
- Trusting the author’s tests, description, or prior self-review as proof.
- Reading Codex first and merely rediscovering its comments.
- Calling different commits an exact-HEAD comparison.
- Reporting raw inline-comment count as defect count.
- Treating lint/style, speculative future needs, or alternative taste as defects.
- Severity inflation without a reachable failure scenario.
- Duplicating one root cause at every symptom.
- Assuming process-local state survives restart or an async error undoes prior effects.
- Copying production transcripts, private identifiers, secrets, or private code into review artifacts.

Arguments: $ARGUMENTS
