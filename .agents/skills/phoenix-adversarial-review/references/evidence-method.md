# Evidence method

This skill was distilled from two empirical sources: internal Phoenix agent-review records and GitHub Codex review findings. The durable artifact contains only generalized review moves.

## Unit of comparison

The valid unit is a **same-HEAD pair**:

```text
(local independent review, repository, base SHA, head SHA)
(Codex review, repository, commit_id = the same head SHA)
```

A pair is invalid for reviewer-gap claims when:

- either SHA is missing or abbreviated ambiguously;
- the local tree was dirty but the dirty patch was not captured;
- Codex reviewed a later or earlier revision;
- the evidence is only a review request, reaction, CI check, branch name, or summary;
- review comments from several commits are counted together.

Keep invalid cases as **unpaired** examples only.

## Normalize and disposition findings

Use one record per semantic defect:

| Field | Meaning |
|---|---|
| `target` | Repository, base SHA, head SHA, clean/dirty state |
| `source` | local or Codex |
| `anchor` | File/symbol or contract; not identity by itself |
| `trigger` | Reachable preconditions |
| `violated_postcondition` | Observable semantic failure |
| `severity` | Impact/reachability |
| `confidence` | Evidence strength |
| `disposition` | validated, disproved, unresolved, or superseded-by-drift |
| `review_move` | Reusable action that would expose the defect |

Two differently worded comments overlap when they identify the same trigger and violated postcondition. Several comments sharing one root cause count as one defect unless they require independent fixes.

Disposition is evidence, not a vote:

- A fix is supporting evidence only after the patch or regression demonstrates the claimed mechanism.
- A closed/rejected thread is not automatically false.
- A reviewer badge or stated severity is not proof of reachability.
- A later clean review does not retroactively validate every earlier fix.

## Avoid anchoring during review

Use prior findings in two phases:

1. **Independent pass:** freeze target, read authorities/diff, generate and falsify candidates without Codex text.
2. **Delta pass:** reveal same-HEAD Codex findings, normalize overlap, validate Codex-only candidates, and extract missing review moves.

This separation tests review capability rather than prompt recall.

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

Do not validate only on cases used to write the probes. Reserve same-HEAD pairs and clean patches for later checks:

- recover known valid defects without revealing them first;
- reject known false or superseded findings;
- produce no actionable findings on a clean patch;
- prove target identity before publishing a delta;
- avoid duplicate root-cause reports and severity inflation.

Report corpus size and limitations. Small or biased samples justify hypotheses, not performance claims.
