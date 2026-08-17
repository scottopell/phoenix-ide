# Sanitized evidence report

This report records the evidence used to derive candidate probes without committing raw production records, private identifiers, or source excerpts.

## Sources and sample sizes

### Internal Phoenix review records

Bounded conversation discovery located review sessions through structural and textual signals such as commissioned reviews, reviewer subagents, frozen-head summaries, and later feedback handling.

- One local/Codex comparison preserved the same head SHA, but did not preserve enough evidence to prove the same base SHA. Under the skill's corrected identity rule it is **unpaired**, not exact-target.
- One additional same-PR lineage candidate contained a local multi-lens review followed by Codex findings, but its local full target identity was not recoverable. It remains **unpaired**, not a near-match.
- These records informed candidate moves around literal parity, incarnation rotation, transition continuity, and recovery. They do not support reviewer-performance rates.

### GitHub Codex review corpus

A bounded API scan covered 150 recent Phoenix pull requests:

- 114 pull requests contained Codex review activity.
- Approximately 4,000 inline review comments were retrieved across repeated review rounds.
- A basic path-and-title normalization produced approximately 3,984 combinations, but that is not semantic root-cause deduplication and must not be reported as a defect count.

Manual inspection of representative comments suggested recurring search areas: transition ordering, authority boundaries, validation, persistence/recovery, failure cleanup, scoped identity, producer/consumer parity, and observer projections. No prevalence or ranking claim is made because comments were neither disposition-labeled nor fully deduplicated by semantic defect.

## Sanitization

Raw query output was kept outside the worktree and deleted after aggregation. This artifact excludes conversation, message, trace, user, tool-call, and private path identifiers; raw prompts; private source excerpts; and database exports.

## Held-out outcomes

A fresh-context adversarial review of the initial skill patch produced three candidate findings:

- one validated finding: tests checked markdown text but not the actual Phoenix skill loader; a loader-level discovery regression was added;
- one process concern already satisfied outside the committed tests: held-out behavior had been exercised directly;
- one overbroad suggestion: regex tripwires cannot prove comprehensive confidentiality and were not presented as doing so.

The exercise demonstrates one recovered defect and false-positive challenge, not a recall or precision estimate. No clean-patch held-out run was preserved with a sealed full base/head target, so the artifact makes no empirical clean-review claim.

## Limitations

- There are currently no verified exact-target local/Codex pairs because earlier records did not preserve both full base and head identities.
- Near-match attribution requires ancestry plus intervening-diff inspection per finding; the sampled lineage candidate did not meet that bar.
- GitHub counts include iterative rounds, superseded comments, possible duplicates, and findings with unknown disposition.
- Theme extraction used bounded keyword grouping plus representative inspection, not independent semantic labeling.
- Internal records and GitHub activity are convenience samples from Phoenix development, not a representative population of repositories, reviewers, or languages.

Accordingly, the corpus-derived probes are unordered hypotheses. Every reported review finding still requires source-grounded falsification in the selected target.
