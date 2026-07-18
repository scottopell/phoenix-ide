---
name: phoenix-explore
description: Breadth-first, read-only investigation of ambiguous Phoenix IDE feedback and requests. Use in Explore mode to trace a raw symptom across easy-to-miss UI, runtime, wire, persistence, tool, worktree, and production boundaries before proposing an implementation task. Produces a bounded evidence-backed task, not code changes.
---

# Phoenix Explore

Turn raw feedback into an execution-ready problem model. Do not implement, edit tracked files, or answer Phoenix behavior from generic prior.

## The loop

1. **Anchor the symptom.** Restate only what the user observed or wants. Preserve exact error text, route, UI label, conversation/task/PR identity, and reproduction conditions. Do not name a cause yet.
2. **Verify the environment.** Confirm checkout/worktree, branch/mode, expected artifacts, and whether the report concerns local or deployed behavior. Stop and ask if the surface is absent.
3. **Map the journey.** Describe user action → visible state → boundary crossings → durable outcome. Mark **verified**, **inferred**, and **unknown** separately.
4. **Choose breadth branches.** Use request cues to open only relevant branches from [references/breadth-map.md](references/breadth-map.md). Inspect the exact artifact first, then one layer on each side of the suspected seam.
5. **Triangulate.** Compare normative spec, implementation, tests, and one independent evidence surface—wire/schema, browser, logs/traces, git history, or persisted state. Seek evidence that disproves the leading hypothesis.
6. **Converge.** Name the failure model, owning invariant/boundary, affected systems, smallest shippable scope, validation journey, risks, and explicit non-goals.
7. **Propose.** After the model is grounded, create one durable task with `taskmd new`, then use the active Explore-mode task-proposal/approval mechanism. Do not begin implementation before approval. Ask focused multiple-choice questions when product preference—not code evidence—determines the design.

## Breadth without wandering

For a substantial investigation, first check only:

- the owning user-facing surface and authoritative spec;
- the producer and consumer on each side of the likely seam;
- existing regression coverage.

Add persistence, wire, reconnect, cancellation, or recovery only when state crosses time/process boundaries or the symptom suggests them.

Then follow cues selectively:

| Feedback cue | Easy-to-miss branches |
|---|---|
| “stuck”, stale, duplicate, vanished, wrong order | UI reducer + SSE replay/dedup + runtime transition/effects + persistence/crash recovery + cancel/retry/stale result |
| loading, timer, retry, silent work | backend phase timestamps + heartbeat/reconnect + provider retry/backoff + sub-agent locality + status derivation |
| button, panel, viewer, shortcut, mobile | owning component + shared primitive + focus/overlay scope + logical vs mounted content + responsive CSS + Ladle/browser journey |
| message/tool output/image/file | typed content path + tool executor + provider capability + persisted child rows + SSE/UI rendering; check for silent drops/parallel representations |
| branch, worktree, task, PR, continue/cleanup | conversation cwd + work scope + all worktrees + checked-out refs + durable task/PR baseline + remote observation |
| restart, resume, recovery, pending | SQLite state + durable workflow/wake path + runtime materialization + UI reconnect + idempotence |
| production-only or intermittent | narrow time window in traces/logs + collector warnings + deployed version/config + process state; local code is only a hypothesis |
| path, reveal, browser/server host | request peer/locality gate + server-vs-browser boundary + remote/proxy behavior + endpoint recheck; reveal containing folder only |
| provider, MCP, malformed/empty stream | exact protocol/error path + provider adapter + capability mapping + retry classification + operation idempotence/side effects + cancel/backoff + parent/sub-agent scope |

The breadth map is a hypothesis generator, not a reading checklist. If a branch cannot change the plan, do not investigate it.

## Evidence discipline

- Search exact symbols/errors first; use conceptual search only when no precise anchor exists.
- Read bounded regions around matches, but follow the relevant arc through its resolution. Setup alone is not evidence of the outcome.
- For production diagnosis, prefer TraceQL at `http://127.0.0.1:10428/select/tempo`, service `phoenix-ide`; use narrow windows/limits and fetch full traces only after identifying trace IDs. Jaeger is fallback.
- Use sub-agents for independent surfaces, not as a substitute for synthesis. Give each a valid worktree, precise question, and disconfirming target.
- Treat a terse user correction as a priority interrupt: acknowledge the miss, restate the corrected target, widen/redirect the map, and re-ground.
- If the same search/tool action fails twice, inspect the environment or hypothesis rather than retrying unchanged.
- Distinguish repository truth from product preference. Ask the user only when multiple valid product choices remain.

## Required output before task proposal

```markdown
## Observed journey
- User action and visible symptom
- Reproduction/environment

## Verified findings
- Fact — evidence anchor

## Inferences and unknowns
- Inference — what would falsify it
- Product question — options, only if needed

## Interaction map
- Producer → boundary → consumer
- Persistence/recovery/cancel/reconnect edges

## Proposed scope
- Owning invariant and smallest fix surface
- Affected systems/files or symbols
- Regression and user-journey validation
- Risks and explicit non-goals
```

A task that only lists files or says “investigate and fix” is not ready. A good task explains the user-visible truth to preserve, likely starting symbols, cross-boundary obligations, acceptance evidence, and what is deliberately out of scope.

## Explore-mode boundaries

Allowed: read/search, bounded production DB queries, logs/traces, browser inspection, tests used as non-mutating diagnostics, temporary files under ignored scratch space, and read-only sub-agents.

Not allowed: editing tracked files, changing task status before proposal approval, committing, destructive Git/DB/process operations, production deploy/restart, or turning an incidental off-path issue into the main task without recording and returning.

Use `phoenix-development` after approval/handoff.

Arguments: $ARGUMENTS
