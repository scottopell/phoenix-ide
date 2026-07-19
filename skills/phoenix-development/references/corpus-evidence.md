# Corpus-derived guidance

This note records why `phoenix-development` and `phoenix-explore` emphasize their current loops. It is an evidence summary, not a raw conversation export or a timeless product specification.

## Method

Analysis used the local production SQLite database at `~/.phoenix-ide/prod.db` read-only.

### Cohort

At analysis time the Phoenix checkout/worktree cohort contained:

- 1,182 conversations from 2026-05-03 through 2026-07-18;
- 635 Explore, 535 Work, 9 Branch, and 4 Direct conversations;
- about 142,000 messages: roughly 75,000 tool results, 62,000 agent messages, and 4,200 user messages.

The corpus is highly concentrated in July. Counts are contextual evidence, not stable product metrics.

### Privacy controls

1. Scope by Phoenix checkout/worktree `cwd`; exclude unrelated projects.
2. Aggregate metadata and typed tool blocks before reading message bodies.
3. Normalize worktree UUIDs and unstable absolute path prefixes.
4. Sample parent conversations by mode, date, subsystem, prompt size, and outcome.
5. Inspect complete relevant arcs through final resolution, not only opening/setup messages.
6. Prefer FTS/bounded SQL to locate candidate arcs. Do not export full bodies, attachments, images, secrets, or broad logs.
7. Record generalized patterns and counterexamples. Do not commit raw quotations or incidental identifiers.
8. Resolve historical behavior against current `AGENTS.md`, normative specs, code, and git history. Current authority wins.

Explicit skill-message counts were not used to measure `phoenix-development`: project-local skill metadata is catalogued separately from invocation, and historical delivery modes varied. Prompt frequency is not evidence that guidance helped.

## Structural evidence

Agent tool-use counts in the Phoenix cohort were approximately:

| Tool | Uses |
|---|---:|
| `read_file` | 22,000 |
| `bash` | 21,100 |
| `search` | 14,000 |
| `patch` | 13,500 |
| `think` | 1,460 |
| `spawn_agents` | 700 |
| browser interactions/profiling | 900 combined |

Command-family extraction found roughly 2,160 `cargo test`, 1,745 `./dev.py check`, 930 `git diff`, and 425 `git commit` tool calls. `./dev.py up` and `restart` appeared only about 20 times each. Commands can contain multiple families, and SQL substring classification can overcount; the order-of-magnitude contrast is the useful fact.

**Implication:** the common workflow is search/read → model/patch → focused test → broad check/diff/commit. Server startup is conditional validation support, not the onboarding centerpiece.

Frequently read surfaces crossed persistence, API, runtime, state machine, workflow, and React boundaries. No single “main file” explains Phoenix behavior.

## Stratified transcript findings

### Development sample

Twelve parent Work arcs across UI, runtime, persistence/contracts, tools, and deployment were inspected through validation, correction, review, and completion.

High-confidence repeated patterns:

- precise failure/invariant modeling before editing (about three quarters of the sample);
- focused regression tests followed by widening validation (about five sixths);
- independent review materially finding missed edge cases in most large changes;
- moving repeated policy to typed/shared boundaries rather than fixing callers independently;
- treating rebase conflicts as semantic integration points;
- classifying broad-suite failures by ownership rather than ignoring or blindly fixing them;
- disposable end-to-end harnesses for high-risk tool/deployment lifecycle work.

Repeated anti-patterns:

- declaring “done” before hostile review and branch-state verification;
- retrying stale text patches instead of rereading and anchoring structurally;
- local call-site patches for policies that belonged in a constructor/type/schema;
- treating a full-check failure as either automatically unrelated or automatically caused by the patch.

Small styling changes were useful counterexamples: they often needed a short local loop, not architectural excavation. The skills therefore select breadth based on boundaries rather than requiring every check for every task.

### Explore sample

Eleven parent Explore arcs were sampled across successful handoffs, idle investigations, user corrections, environment mismatch, production incidents, and short raw feedback.

High-confidence repeated patterns:

- strongest explorations framed unknowns, verified the exact artifact, crossed relevant system boundaries, distinguished fact from assumption, and converged to a bounded task;
- specific behavior/error questions answered from generic prior were the clearest failure mode;
- terse user corrections often carried the highest-value signal and successfully redirected the investigation when treated as priority interrupts;
- early repository/worktree/tool-context verification prevented wasted work;
- sub-agents improved breadth only when their findings were synthesized into one problem model;
- broad conceptual inquiry without a stopping rule produced insight but weak handoff.

**Implication:** Explore should optimize for evidence-backed convergence, not maximum reading or fastest plausible answer.

## Authority reconciliation

Corpus practices were retained only when consistent with current repository authority. Important corrections to historical practice include:

- spEARS v2 roles: requirements/Allium are normative, ADRs preserve rationale, executive docs report current status;
- Rust SSE types generate TypeScript; generated files are never hand-edited;
- persisted addressable structure is normalized into schema/rows;
- task filenames are allocated and transitioned with `taskmd`;
- commits are routine on owned branches;
- production traces/log warnings outrank assumptions based on a local checkout.

## Evaluation approach

Sanitized historical scenarios live in `skills/phoenix-explore/references/evaluation-scenarios.md`. Drafting used the calibration set; the holdout set was evaluated only after the initial skill drafts.

The comparison asks whether guidance causes an agent to identify:

1. the exact artifact and user journey;
2. authoritative spec/code/test anchors;
3. producer/consumer and durability/wire boundaries;
4. disconfirming evidence and uncertainty;
5. minimum proof and a bounded task;
6. unnecessary branches that add no decision value.

This evidence note should be refreshed when repository practices or the production corpus materially change. Do not mechanically add every frequent behavior to the skills; frequency can encode repeated mistakes.
