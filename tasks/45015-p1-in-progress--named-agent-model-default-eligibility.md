# Configure named workers and usable execution choices

## Approved scope

Implement the user-reaffirmed direction from [design PR #705](https://github.com/scottopell/phoenix-ide/pull/705), extended by the September design discussion. That PR was closed for roadmap capacity and ownership; its product direction remains accepted. [Roadmap #651](https://github.com/scottopell/phoenix-ide/issues/651) is the coordination authority.

Production evidence motivating this work: a four-worker spawn with omitted model overrides failed because a filesystem agent's hidden default requested an unavailable provider model. Explicitly selecting a registered model admitted the retry. The new catalog must prevent that predictable failure before advertising a worker.

- One versioned XDG `phoenix-ide/config.toml`, with inline named-worker instructions.
- Filesystem named-agent discovery retires; Skills retain portable expertise. Plugin hosting remains separate.
- A named worker is optional and supplies instructions plus optional ordered execution candidates.
- Generic workers can request a configured tier or an exact model/connection/effort choice.
- Exactly one execution selector; no implicit mixture of tier, exact model, and inherited effort.
- With no worker preference or selector, inherit the parent's model, connection, and effort. No configurable default tier.
- Candidates bind model, configured connection, and effort. Missing routes permit trying the next candidate; incompatible effort on a reached available candidate is a configuration diagnostic, not silent substitution.
- Only available workers, tiers, routes, and supported explicit efforts appear in callable choices. Exact selection never silently falls back.
- Permission is requested separately; existing parent/workspace admission guards remain.
- Resolve from one request catalog shared by tool advertisement and admission. Config loads per parent runtime; route availability refreshes at request boundaries.
- Persist admitted child instructions and execution choices through existing creation/persistence boundaries. Existing recovery guarantees remain unchanged.

## Complexity limits

Reuse the existing model registry and backend connection slots. No second router, endpoint/account fingerprint framework, config watcher, health probes, provider retry system, legacy/new catalog merge, migration engine, new child workflow, or broader parallel-writer support. Connection names identify configured backend slots, not immutable accounts/endpoints across operator reconfiguration.

## Acceptance evidence

- Codex-only registry: worker preferring unavailable Opus then usable Sol advertises and selects Sol; Opus-only worker is excluded with a diagnostic.
- Generic omission inherits parent model/connection/effort for either requested authority.
- Named instructions survive a tier or explicit execution override; omitted effort on explicit model selection uses the model's native default.
- Wrong connection and unsupported effort reject; no child starts from an invalid batch.
- Config changes cannot silently rewrite existing child instructions/execution; request catalog and admission agree.
- Selected child effort, persona, and connection commit together; explicit omitted effort never leaks the parent's effort.
- Retired filesystem files do not enter the active catalog and remain untouched; migration guidance explains manual transfer.
- Focused tests, owning modules, Allium/spec checks, full `./dev.py check`, multiple immutable local adversarial reviews, then fresh Codex review on the published PR.

## Integration

Worktree: `phoenix-agent-config-45015`; branch `codex/45015-agent-execution-config`. Source on essmbp; heavy validation on devmbp because the roadmap's laptop disk gate is active. The next ADR/migration numbers are taken from current main; reconcile numbering on rebase if concurrent PRs merge first rather than creating dependency placeholders.

## Separate follow-ups

Restart repair's synthetic errors on legitimate terminal submissions and other spawn authority/workspace rejection classes remain outside this feature. Their evidence remains in the local production analysis report. Do not use inflated historical error counts to expand this change into a recovery redesign.
