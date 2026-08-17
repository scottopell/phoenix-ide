---
name: phoenix-adversarial-review
description: Adversarially review an agent-authored diff, PR, commit, or implementation by trying to falsify its correctness before merge. Use when asked for a rigorous code review, independent challenge, exact-HEAD review, self-review blind-spot check, or comparison with Codex findings. Especially useful for Phoenix changes involving invariants, persistence, wire contracts, concurrency, recovery, security boundaries, tests, or unnecessary complexity.
argument-hint: <diff-or-pr-scope>
---

# Phoenix Adversarial Review

Try to break the change, not explain it. A useful review produces a small set of source-grounded defects—or says that no candidate survived falsification.

Repository `requirements.md` files and `.allium` specs are normative behavior authorities. ADRs are authoritative decision history and doctrine. Task/PR intent supplies scope and non-goal context; it does not override those authorities. Prior reviews, production records, and Codex findings are evidence, never requirements.

## 1. Freeze the review target

Before reading conclusions from any prior review:

1. Select the target kind: an immutable commit/range, or a working-tree snapshot.
2. Record the repository plus the target's full base and head SHAs. For a working-tree target, also capture the staged and unstaged patch and cleanliness; unrelated worktree state does not affect an immutable commit/range target.
3. Refuse an **exact-target** comparison unless both reviews used identical base and head SHAs. Equal heads with different or unknown bases are not exact: retargeting or base advancement changes the reviewed diff. A branch name, green CI run, request timestamp, or later summary does not prove identity.
4. If the selected target includes a dirty worktree, capture that exact snapshot or stop and request a committed target. Never silently mix committed and uncommitted code.
5. Fetch the complete diff and changed-file list. Detect truncation, generated files, renames, migrations, and changes outside the apparent feature directory.

When targets differ, do not infer a reviewer blind spot until ancestry and the intervening diff prove the specific defect already existed in the locally reviewed snapshot. Label that weaker evidence **near-match** with direction and commit distance; otherwise label it **unpaired**.

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
- what can outlive the request, process, worktree, or deployment;
- which facts committed SQLite rows and durable time own, versus which process-local runtimes, tasks, timers, kicks, queues, SSE streams, caches, and UI views merely project;
- which outcomes are local SQLite authority results versus genuinely ambiguous external outcomes with feature-owned recovery contracts.

Do not review only the edited lines. Trace one layer upstream and downstream of every changed contract.

## 3. Run adversarial probes

Select probes from [references/probes.md](references/probes.md) based on the blast-radius map. These are hypothesis generators, not a checklist quota.

Always perform these gap-informed moves when applicable:

- **Producer/consumer literal check:** compare discriminants, marker strings, field names, enum cases, encodings, and defaults at the exact write and read sites. Do not infer parity from shared terminology.
- **Incarnation check:** follow identity creation through restart, respawn, retry, rekey, migration, and reuse. An identity assigned correctly at construction may become stale when the resource is replaced in place.
- **Recovery without process identity check:** erase process-local runtimes, tasks, timers, kicks, queues, SSE streams, and caches mentally. Verify another process can discover unfinished obligations from committed SQLite rows and durable time; process-local continuity may improve latency but must be disposable.
- **Commit-before-publication check:** distinguish a privately proposed transition from adopted committed state. For direct-turn semantic state, verify the owning SQLite transaction commits materialization and reducer projection before observer publication or use by routing/admission; stale, replayed, or failed materialization must not leak the proposal.
- **Local-authority loss check:** when an authoritative same-process SQLite command returns no typed result, permit at most one exact classification query against the rows owning the needed fact. If it cannot classify the fact, require process fail-stop without semantic uncertainty state, continued admission/publication, or cleanup through the suspect persistence path. Do not apply this rule to genuine external ambiguity.
- **One-authority check:** look for two representations, selectors, caches, timestamps, or status fields claiming the same fact.
- **Error-path data check:** follow observations and state changes through `Err`, timeout, stale, duplicate, partial-success, and cleanup paths—not only returned success values.
- **Test oracle check:** mutate the behavior mentally. Would the test fail for the right reason, or only prove that a mock was called / an error occurred?

For timer, retry, wake, cancellation, or concurrent code, trace causal completion, durable ownership, and publication ordering. A sleep or timeout is not synchronization. If a task owns the exact local SQLite authority boundary, panic, unexpected exit, or cancellation without a typed result selects the same exact-query-or-fail-stop rule; ordinary task failure remains feature-owned.

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

## 5. Compare local review with Codex only after an isolated review

Run the independent pass in a fresh context that has not received external findings. Give it only the frozen target, authorities, and review instructions, then seal its output before loading Codex feedback. If findings already appeared in the current context and fresh isolation is unavailable, label the pass **anchored** and do not use it to measure overlap or reviewer blind spots.

Normalize findings by semantic defect rather than wording or line number. Record two independent axes:

**Evidence tier** describes target comparability:

- **exact-target:** identical full base and head SHAs;
- **near-match:** different targets in one proven lineage, with the intervening diff showing the specific defect already existed locally;
- **unpaired:** target identity, ancestry, or defect continuity is not proven.

**Comparison outcome** describes who found the semantic defect:

- **overlap:** both found the same violated postcondition;
- **local-only:** local review alone found it;
- **Codex-only:** Codex alone found it;
- **disputed:** concrete counterevidence defeats or materially changes one review.

A finding can be both `near-match` and `Codex-only`; count it once in each axis, never twice as two defects. Use broader Codex review trends only to propose probes. Deduplicate repeated rounds and root causes, validate representative patch context, and never describe a corpus-only trend as a local-review miss.

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
- Frozen target: immutable range or captured working snapshot; full `<base>..<head>`; exact-target status.
- Authorities and boundaries inspected.
- Tests/reproductions run and material limitations.

## Review delta
- Evidence tier: exact-target N; near-match N; unpaired N; anchored N.
- Comparison outcome: overlap N; local-only N; Codex-only N; disputed N.
- Missing review moves learned from valid Codex-only findings.
```

If nothing survives falsification, say **“No actionable findings.”** Still report the frozen target, inspected boundaries, tests run, and limitations. Never manufacture a finding to make the review look productive.

## Anti-patterns

- Summarizing the PR instead of challenging it.
- Trusting the author’s tests, description, or prior self-review as proof.
- Claiming independence when external findings were already visible in the review context.
- Calling equal heads with different or unknown base SHAs an exact-target comparison.
- Rejecting an immutable commit target because the unrelated current worktree is dirty.
- Reporting raw inline-comment count as defect count.
- Calling a same-PR review a near-match without inspecting ancestry and the intervening diff.
- Treating Codex-wide trends as evidence that local reviewers missed those defects.
- Treating task or PR wording as behavior authority over requirements, Allium, or ADR doctrine.
- Publishing or adopting proposed direct-turn state before its owning materialization transaction commits.
- Inventing conversation/workflow uncertainty, in-process runtime replacement, or cleanup after an unclassified local SQLite authority failure instead of exact-query-or-fail-stop.
- Applying local SQLite fail-stop doctrine to genuine external ambiguity governed by a feature-owned recovery contract.
- Treating lint/style, speculative future needs, or alternative taste as defects.
- Severity inflation without a reachable failure scenario.
- Duplicating one root cause at every symptom.
- Assuming process-local state survives restart or an async error undoes prior effects.
- Copying production transcripts, private identifiers, secrets, or private code into review artifacts.

Arguments: $ARGUMENTS
