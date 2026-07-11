# Snapshot conversation system prompts for cache stability

Phoenix rebuilds the system prompt for every request, so changes in repository guidance, task inventory hints, skills, or generated mode context can mutate the leading cached prefix in the middle of a conversation. The executor contains a dangling `TODO(task 61006)` but no matching task artifact. Define and implement a persisted per-conversation system-prompt snapshot with an explicit refresh lifecycle so stable conversations retain byte-identical prefix content.

Acceptance criteria:
- [ ] The initial effective system prompt is persisted as normalized schema, not hidden in an unrelated JSON blob.
- [ ] Every request in a transcript generation uses the persisted snapshot rather than recomputing mutable guidance.
- [ ] Explicit refresh/restart behavior creates a clear transcript-generation boundary and cache rewarm point.
- [ ] Explore/work mode transitions and task approval preserve required mode semantics without silent prefix mutation.
- [ ] Tests mutate guidance, tasks, and skills between turns and prove the active snapshot remains byte-identical.
- [ ] Recovery and migration behavior for pre-snapshot conversations is explicit and lossless.
- [ ] The dangling numeric TODO is replaced with a durable local fact or normative spec reference.
- [ ] Cache-read measurements compare pre/post behavior across a mid-session guidance mutation.
- [ ] Specs describe snapshot lifecycle and generation boundaries.
- [ ] `./dev.py check` passes.
