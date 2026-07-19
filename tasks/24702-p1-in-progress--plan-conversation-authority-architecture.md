# Plan explicit conversation authority and runtime posture

Produce a complete, implementation-ready architectural plan for allowing an Explore conversation to request and receive the full Work toolset while remaining in the Explore phase. This task is planning and design work only; it does not implement the feature.

## Starting point

Treat PR #530 and `feat/explore-work-tools` as discarded implementation history. They may inform failure-mode analysis but are not an implementation base.

Begin by:

1. Reading review comments authored by `scottopell` and the relevant changes on PR #485. Do not inventory or process automated/bot review comments unless a `scottopell` comment explicitly points to one as relevant.
2. Studying the durable workflow engine as it lands, including its persistence, effect ordering, recovery, user-decision, and runtime reconstruction contracts.
3. Inspecting the user's other active stacks and branches that affect conversation lifecycle, WorkScope, multiple PRs/tasks per conversation, runtime roles, sub-agents, permissions, and sandbox enforcement.
4. Re-reading the normative permissions, bedrock, projects, sub-agent, work-lifecycle, PR-association, chains, and Coordinator specifications against the newly landed architecture.

Use production traces where they clarify deployed behavior. Local VictoriaTraces is available at `127.0.0.1:10428`; prefer bounded TraceQL queries and inspect full traces only after identifying relevant trace IDs.

## Architectural premises to validate

Do not blindly preserve these as conclusions; test them against the landed code and active stacks:

- A conversation chain has one lifecycle in which Explore precedes Work; an investigation-authority decision occurs within Explore and is not task approval.
- Durable user-granted authority is a missing explicit input to runtime posture.
- Effective tools and enforcement derive from user authority, environment relationship, runtime role, platform capability, and a non-overridable safety profile.
- The primary environment distinction is Phoenix-allocated/owned WorkScope versus an arbitrary unowned host location versus no coding environment.
- A WorkScope owns the physical worktree and work-affine resources. Tasks, branches, commits, and PRs are plural artifacts within that environment rather than conversation identity.
- Runtime role is an intentional exhaustive dimension: user conversation, sub-agent, or singleton Coordinator.
- Work-sub-agent exclusivity is coordination policy, not intrinsic Work authority.
- Nono is an execution enforcement mechanism beneath the durable authority model, not the authority source of truth.

## Required plan coverage

The final plan must define, with concrete code and spec anchors:

- the conceptual model and legal state combinations;
- the boundary among lifecycle phase, operational workflow state, durable authority, runtime role, environment ownership, platform support, and safety policy;
- how the durable workflow engine represents the authority request, user decision, blocked state, effect ordering, crash consistency, restart, runtime eviction, and post-decision continuation;
- how model-visible registries, MCP exposure, execution sandbox/policy, prompts, and prompt inspection derive from one effective runtime posture;
- how restricted Explore uses Nono and how approved full operational authority selects matching Work-level execution without implying implementation approval;
- whether authority is chain-scoped or conversation-instance-scoped, its lifetime, inheritance, rejection semantics, and any revocation boundary;
- transcript validity and history replay when the advertised registry expands;
- context-threshold/continuation ordering around the blocking request;
- parent/sub-agent inheritance, immediate child construction, resumed child reconstruction, cwd/worktree containment, and future compatibility with concurrent Work sub-agents;
- allocated WorkScope versus unowned cwd versus no-environment roles, including Coordinator and future pure-chat compatibility;
- UI approval state, reload and cross-tab reconciliation, composer/chat conflict behavior, cancellation, stable outcomes, notifications, analytics, and exhaustive state consumers;
- database/schema changes and atomic persistence boundaries, following the rule that persisted structure belongs in relational schema rather than JSON blobs;
- required spEARS, Allium, ADR, executive, generated wire-type, unit, property, integration, recovery, and end-to-end test changes;
- explicit sequencing into reviewable implementation slices that remain correct and shippable at each step;
- risks, open design decisions, and clearly separated follow-up debt.

## Scope discipline

The plan must explicitly decide what is required for this feature versus follow-up architecture work. In particular, evaluate—but do not automatically include—retiring `ConvMode::Branch`, moving environment ownership out of `ConvMode`, simplifying Explore→Work to begin from a detached recorded base commit, plural task-file handling, broader Nono coverage, safety rules for sensitive host files, and concurrent Work sub-agents.

Do not create a general-purpose per-tool approval/token system unless the evidence demonstrates that a small typed authority posture cannot satisfy the requirements.

## Deliverable

Create a durable planning artifact suitable for a subsequent implementation task or task series. It must include:

1. a current-state architecture map;
2. findings from PR #485, the durable workflow engine, active stacks, relevant history, specs, and traces;
3. the recommended target model with alternatives and rationale;
4. state/effect/persistence and recovery diagrams;
5. an exhaustive consumer-impact matrix;
6. an implementation sequence with verification obligations;
7. resolved design decisions and any questions that genuinely require user choice.

Use the asking-questions discipline during planning: ask the highest-leverage behavioral question first, condition later questions on prior answers, and stop only when another answer would no longer reshape the plan.
