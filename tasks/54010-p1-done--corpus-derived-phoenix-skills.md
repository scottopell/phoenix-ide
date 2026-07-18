# Derive Phoenix development and exploration skills from production practice

Replace the stale `phoenix-development` skill and add a `phoenix-explore` skill using evidence from the local Phoenix production conversation corpus, current repository guidance, specs, code, and recent development history.

The goal is not to make either skill an encyclopedia. They should provide high-leverage onboarding, route agents toward the right authoritative material, expose easy-to-miss cross-system interactions, and use progressive disclosure so commonly loaded guidance saves tokens rather than consuming them.

## Outcomes

- `phoenix-development` becomes a concise operational home base for implementation work: orientation, the normal edit/validate/commit loop, source-of-truth routing, common failure recovery, and handoffs to specialized skills.
- `phoenix-explore` becomes a breadth-first investigation guide for raw user feedback that may describe only one visible symptom. It helps an Explore-mode agent discover the complete interaction surface before proposing work.
- Detailed or lower-frequency material lives in focused reference files rather than an oversized `SKILL.md`.
- Every substantial rule has traceable support from current authoritative guidance or repeated production-corpus evidence.
- Raw production conversation content, paths containing incidental user data, and secrets are never committed.

## Corpus analysis

Use `~/.phoenix-ide/prod.db` as the sole conversation-history source. Do not use the obsolete Claude Code history locations or event model.

1. **Define the cohort**
   - Select conversations rooted in the Phoenix checkout and Phoenix-owned worktrees.
   - Record aggregate corpus size, date range, conversation modes, message/tool distribution, and relevant schema/version facts.
   - Separate parent work, Explore/task-proposal work, implementation work, sub-agent work, and operational/release work where the persisted metadata permits it.

2. **Analyze structure before bodies**
   - Derive tool sequences, files/specs visited, command families, retries, errors, test progression, task transitions, and completion shape from typed message content and metadata.
   - Normalize worktree UUIDs and other unstable paths before aggregation.
   - Distinguish injected project guidance and repeated boilerplate from behavior that emerged during successful work; frequency in prompts is not evidence of usefulness.

3. **Sample transcript content deliberately**
   - Use FTS and bounded SQL queries to locate candidate conversations, then inspect complete relevant arcs through their resolution tails rather than isolated opening messages.
   - Stratify across time, subsystem, task size, conversation mode, and outcome so recent conventions do not erase older anti-patterns and one prolific workflow does not dominate.
   - Prioritize user corrections, reversals, failed checks, rework, missed subsystem discoveries, successful verification, and concise first-pass orientation.
   - Keep an evidence ledger containing sanitized pattern names, prevalence, confidence, representative conversation IDs/timestamps, counterexamples, and the proposed skill implication. Do not store transcript quotations in the repository unless they are both necessary and safely generalized.

4. **Synthesize patterns, not anecdotes**
   - Identify practices that reduce search/read/tool churn, prevent rework, and improve first-pass validation.
   - Identify anti-patterns such as premature narrowing, reading implementation without its normative spec, treating the first visible UI surface as the whole feature, missing wire/persistence/codegen boundaries, using generic commands instead of project wrappers, and validating only the changed component.
   - Resolve corpus evidence against current `AGENTS.md`, normative specs, repository code, and git history. Current normative rules win over historical behavior; contradictions are recorded as obsolete practice rather than taught.

## Skill design

### `phoenix-development`

Keep the top-level skill short and imperative. It should include:

- a one-screen “start here” development loop;
- how to locate the owning crate/UI surface and its authoritative spec before editing;
- the current `./dev.py` workflow, gated/full check behavior, codegen, targeted tests, dev/prod boundaries, and commit expectations;
- high-value correctness constraints that repeatedly prevent rework, expressed as decision rules rather than duplicated policy prose;
- a routing table to task tracking, Rust, React, browser QA, Allium/spEARS, deployment, release, crate extraction, and performance skills;
- recovery paths for common failures and stale assumptions;
- links to focused references for detailed commands or subsystem-specific procedures.

Remove stale anchors such as pre-workspace tool paths, incomplete server commands, and obsolete assumptions about how conversation testing or specs work.

### `phoenix-explore`

Design this for the initial read-only phase where the input is raw user feedback, not a prepared implementation brief.

Its core loop should be:

1. Restate the observable symptom without prematurely naming the cause.
2. Identify the user journey and all state/boundary crossings involved.
3. Build a bounded interaction map from cues in the request.
4. Triangulate each likely surface across normative behavior, implementation, persistence/wire boundaries, and tests/observability.
5. Seek disconfirming evidence and inspect resolution paths, not only the happy path.
6. Produce a scoped task proposal that names affected systems, invariants, validation, and explicit non-goals.

Provide a cue-driven breadth matrix for easy-to-miss Phoenix seams, including where relevant:

- React/XState state and backend state-machine transitions;
- runtime effects, cancellation, retry, stale-result, and sub-agent boundaries;
- persistence schema/migrations and crash recovery;
- SSE wire types, replay/deduplication, generated TypeScript, and UI reducers;
- worktree/branch ownership, task lifecycle, and conversation cwd;
- browser/MCP/provider boundaries and server-vs-browser locality;
- keyboard/focus/viewer overlays and responsive UI behavior;
- production traces/log warnings when diagnosing deployed behavior;
- specs, ADRs, executive status, and existing regression coverage.

The matrix must be selective rather than a mandate to read the entire repository: request cues choose branches, while a small cross-boundary sanity pass guards against tunnel vision.

## Validation

- Validate skill discovery/frontmatter and any repository skill checks.
- Run `./dev.py check` with appropriate gating for changed files.
- Build a sanitized evaluation set from representative historical *initial user requests* across UI, runtime, persistence, tool, worktree, and operational tasks. Do not commit raw private transcript text; paraphrase and remove incidental identifiers.
- Compare the old and new guidance on:
  - time/tool calls to identify authoritative files;
  - interaction surfaces found before proposing work;
  - false-positive breadth and unnecessary reads;
  - missed specs, persistence/wire/codegen, tests, or operational boundaries;
  - correctness and actionability of the resulting plan.
- Use at least one holdout set not consulted while drafting the skills. Revise only when evaluation failures reveal a generalizable rule.
- Review both skills for duplication with `AGENTS.md` and sibling skills. Prefer routing and compact decision rules over copying large policy blocks.

## Deliverables

- Updated `.agents/skills/phoenix-development/SKILL.md` and focused references as justified.
- New `.agents/skills/phoenix-explore/SKILL.md` and focused references as justified.
- A privacy-safe analysis methodology and evidence summary sufficient for future maintainers to understand why the skills say what they say; no raw corpus export.
- Sanitized evaluation scenarios and results, in the lightest maintainable form supported by the repository.
- Remove or clearly supersede stale development-skill guidance discovered during the work.

The obsolete user-level `cc-history-query` skill is not an input to this design. Its only reusable principle is targeted search followed by inspection of the resolution tail; its Claude-specific storage paths and parsing model must not be carried forward. If deletion of that user-level skill is desired, handle it separately from the repository commit so this task does not silently modify configuration outside the worktree.
