# Evidence method

This skill was distilled from two empirical sources: internal Phoenix agent-review records and GitHub Codex review findings. The durable artifact contains only generalized review moves.

## Evidence tiers

Keep three tiers separate in analysis and reporting.

### Tier 1: exact-target review pair

The strongest unit is:

```text
(isolated local review, repository, full base SHA, full head SHA)
(Codex review, repository, the same full base SHA, the same full head SHA)
```

Both identities matter: equal heads can represent different reviewed diffs after retargeting or base advancement. Use this tier to attribute a finding to a local or Codex review gap.

### Tier 2: lineage-confirmed near match

A local review at commit `L` and Codex review at descendant commit `C` may be compared only when the intervening diff `L..C` is inspected. A Codex finding can inform a local-review gap when all are true:

- `L` and `C` are proven commits in the same PR lineage;
- the locally reviewed snapshot was clean or captured exactly;
- the finding's faulty lines or semantic mechanism already existed at `L`;
- no intervening commit introduced, removed, or materially changed the trigger, guard, or consumer;
- the record states the direction and commit distance.

Label this **near-match**, never exact-HEAD. Compare individual findings, not whole-review counts. If ancestry or defect presence is uncertain, classify it as unpaired.

### Tier 3: corpus trend

Codex findings without a local counterpart can reveal candidate review moves but cannot reveal self-review misses. Derive trends from semantic findings, not raw comment totals. Deduplicate repeated review rounds and comments that share one root cause. Validate representative examples against their patch context before promoting a trend into a probe.

A record is invalid for reviewer-gap claims when:

- either reviewed target is missing or abbreviated ambiguously;
- the local tree was dirty but the dirty patch was not captured;
- evidence is only a review request, reaction, CI check, branch name, or summary;
- comments from several commits are counted together without lineage inspection;
- a later finding is assumed to predate the intervening patch without checking it.

Keep these cases as **unpaired** examples only.

## Normalize and disposition findings

Use one record per semantic defect:

| Field | Meaning |
|---|---|
| `target` | Repository, full base SHA, full head SHA, immutable range or captured working snapshot |
| `source` | local or Codex |
| `anchor` | File/symbol or contract; not identity by itself |
| `trigger` | Reachable preconditions |
| `violated_postcondition` | Observable semantic failure |
| `severity` | Impact/reachability |
| `confidence` | Evidence strength |
| `disposition` | validated, disproved, unresolved, or superseded-by-drift |
| `evidence_tier` | exact-target, near-match with distance/direction, corpus trend, or unpaired |
| `comparison_outcome` | overlap, local-only, Codex-only, or disputed |
| `independence` | isolated or anchored |
| `review_move` | Reusable action that would expose the defect |

Two differently worded comments overlap when they identify the same trigger and violated postcondition. Several comments sharing one root cause count as one defect unless they require independent fixes.

Disposition is evidence, not a vote:

- A fix is supporting evidence only after the patch or regression demonstrates the claimed mechanism.
- A closed/rejected thread is not automatically false.
- A reviewer badge or stated severity is not proof of reachability.
- A later clean review does not retroactively validate every earlier fix.
- Task text, PR descriptions, review dispositions, and landed patches supply context or evidence; they do not override normative `requirements.md`, `.allium`, or accepted ADR doctrine.

## Isolate before external findings enter context

Use prior findings in two contexts:

1. **Isolated pass:** start a fresh context with only the frozen target, authorities, and review instructions; seal its findings before retrieving or injecting external feedback.
2. **Delta pass:** in a separate synthesis context, reveal exact-target or lineage-confirmed Codex findings, normalize overlap, validate Codex-only candidates, and extract missing review moves.

Instructions cannot erase conclusions already present earlier in a conversation. If isolation is unavailable, mark the review anchored and do not use its overlap as capability evidence.

## Confidentiality

Production records are queried with minimum necessary scope. Never commit or reproduce:

- conversation, message, trace, user, or tool-call identifiers;
- secrets, credentials, private paths, personal data, or raw prompts;
- private repository names or source excerpts;
- full transcripts or database exports.

Allowed durable evidence is aggregate and sanitized:

- number of bounded records sampled;
- broad defect classes;
- generalized review moves;
- public GitHub URLs when useful;
- explicit access and sampling limitations.

Keep raw query output outside the worktree and delete it when no longer needed.

## Held-out validation

Do not validate only on cases used to write the probes. Reserve exact-target pairs, lineage-confirmed near matches, and clean patches for later checks:

- recover known valid defects without revealing them first;
- reject known false or superseded findings;
- produce no actionable findings on a clean patch;
- prove target identity before publishing a delta;
- avoid duplicate root-cause reports and severity inflation.

Report corpus size and limitations. Small or biased samples justify hypotheses, not performance claims.
