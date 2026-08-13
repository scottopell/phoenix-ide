# Design a general high-effort, low-cost coding-worker capability

Explore and design how Phoenix should support coordinator-directed coding workers that pair a comparatively inexpensive or fast model with elevated reasoning effort, without introducing a special-case `coding-agent` or coupling the feature to one model such as `gpt-5.6-terra`.

## Goal

Produce an evidence-backed design that decomposes the concept across Phoenix's conversation/sub-agent runtime, provider/model configuration, instruction composition, task delegation, validation, concurrency, tool access, and Codex's agent model. The result should explain which concepts belong in Phoenix versus which should remain provider-specific, and should support candidates such as Terra and GLM 5.2 as well as future models.

## Investigation

- Trace Phoenix's current coordinator and spawned-agent lifecycle end to end: agent discovery, persona/config resolution, model and reasoning-effort selection, instruction construction, worktree/write-access semantics, tool registry, concurrency limits, persistence, recovery, and result handoff.
- Read the normative Phoenix specs and ADRs governing sub-agents, tools, model selection, and task/worktree execution before recommending changes.
- Inspect upstream `codex-rs` as needed to understand its agent/thread/task abstractions, role and instruction layering, reasoning controls, validation loops, delegation behavior, and any assumptions that should not leak into Phoenix's provider-neutral domain model.
- Compare the Codex agent model with Phoenix's state-machine-driven agent model. Identify semantic overlaps, mismatches, and the appropriate translation boundary.
- Decompose the supplied prior art into independent capabilities rather than copying its configuration shape:
  - semantic worker role/capabilities;
  - model suitability and cost/speed/quality policy;
  - reasoning-effort policy;
  - well-defined task contract;
  - deterministic acceptance/validation contract;
  - coordinator guidance for delegation versus direct execution;
  - concurrency and resource budgets;
  - write isolation, permissions, and failure/retry behavior.
- Evaluate whether existing named personas, generic sub-agent configuration, skills, task briefs, or a typed delegation contract can express the idea already, and identify the smallest general extensions where they cannot.
- Explicitly distinguish model identity from worker role. Avoid model-name conditionals and avoid encoding subjective labels such as “cheap” into persistent domain types unless backed by configurable policy or capabilities.
- Consider provider differences, including providers that do not expose Codex-style reasoning effort, and define graceful capability handling with visible logging rather than silent omission.
- Identify deterministic validation options (tests, checks, acceptance commands, structured predicates), how validation is authorized and executed, and what happens when no deterministic validator can be supplied.
- Assess coordinator UX and prompt ergonomics: when a worker should be suggested or selected, how users override it, and how Phoenix prevents unnecessary delegation for trivial tasks without relying solely on brittle prompt prose.

## Deliverable

Create a concrete implementation plan, not the implementation itself. Include:

1. A concept model and terminology that remain provider- and model-neutral.
2. Current-state and proposed-flow diagrams covering coordinator → worker → validation → result.
3. A Codex/Phoenix responsibility matrix and an explicit provider translation boundary.
4. At least two viable architecture options, with trade-offs and a recommended minimal path.
5. Proposed typed configuration/API shapes, including capability fallback behavior, while preserving correct-by-construction constraints.
6. Persistence and crash-recovery implications.
7. Security, tool-permission, worktree, concurrency, retry, and cost-control implications.
8. An incremental delivery and test strategy tied to user-visible value.
9. Open product choices presented as concrete alternatives for user decision; do not leave unresolved ambiguity buried in prose.
10. Relevant upstream `codex-rs` citations pinned to stable files/symbols or revisions where practical.

## Constraints

- Keep the design general across models and providers; Terra and GLM 5.2 are examples, not special cases.
- Do not create parallel representations for model, role, validation, or execution-policy semantics.
- Make invalid delegation states structurally unrepresentable where practical—for example, if a coding worker requires a deterministic validation contract, represent that requirement in types rather than developer-instruction convention alone.
- Preserve Phoenix's durable state-machine and effect boundaries; do not bypass them with an ad hoc agent loop.
- Treat upstream Codex behavior as provider/runtime evidence, not automatically as Phoenix's desired domain model.
- Call out which conclusions are confirmed by code/spec evidence and which are recommendations.
