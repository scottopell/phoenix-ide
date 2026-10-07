# Global Coordinator subagent support — user design interview first

## Queue and admission boundary

Queued P1; design required. The user requested: “queue up a p1 task to add subagent support for global coord. This will need a design discussion/interview with me to design scope of subagents and how work scopes would work etc.”

The first milestone is a Global-led interview with the user and an agreed minimal design. This task does not admit implementation, launch a worker, expand permissions, or interrupt existing delivery. Global's current exposed tool surface does not include `spawn_agents`; ordinary coding conversations having that tool does not grant it to Global.

## Interview questions, not predetermined requirements

- Which concrete delegation journeys matter first: bounded reads, review, authenticated API operations, coding, or a smaller subset?
- Which tools and authentic skills may a child receive? How should authenticated skill composition work without copied skill text becoming authority?
- Does a child need a WorkScope, whose authority and environment does it bind, and how are per-command targets selected when Global has no WorkScope of its own?
- How are read/write ownership and conflicts with existing ordinary workstreams represented?
- What lifetime, cancellation, restart, and continuation behavior is actually needed?
- Which results, provenance, and visible UI make delegation understandable?
- How should model, reasoning effort, standard/fast tier, defaults, and cost be selected and disclosed?
- How is accidental default filesystem/project selection or silent privilege inheritance prevented?

## Required first milestone

- [ ] Global conducts the design interview with the user when ready, outside unrelated active streams.
- [ ] Record agreed minimal journeys, authority/environment boundaries, ownership, lifecycle, model settings, and acceptance evidence in the appropriate spec/decision artifacts.
- [ ] Resolve substantive scope choices with the user before admitting a bounded implementation task.

No framework, scheduler, blanket tool inheritance, implicit project, or new Global powers are selected by this brief. Implementation remains uncommissioned until the interview and design agreement.
