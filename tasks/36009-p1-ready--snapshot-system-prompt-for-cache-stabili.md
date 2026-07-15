# Snapshot project instructions for cache stability

Phoenix rebuilds repository guidance and the skill catalog for every request, so filesystem changes can silently alter agent behavior and invalidate a large cached prefix in the middle of a conversation. A live GPT-5.6 Sol probe recorded in `docs/research/codex-token-efficiency-hunt.md` showed a one-line `AGENTS.md` mutation reducing cached input from 17,152 to 5,888 tokens while uncached input rose from 796 to 19,287 tokens.

Implement a persisted, versioned project-instruction bundle with an explicit user-controlled refresh lifecycle. Do not freeze the entire effective system prompt: Explore/Work mode, permissions, tool availability, task approval, and other authoritative runtime state remain live and structurally separate from the project bundle.

## Product decisions

- The versioned project bundle contains discovered `AGENTS.md` / `AGENT.md` guidance and the available-skill catalog (names and descriptions).
- The active bundle never changes merely because source files change. Phoenix shows that newer project instructions are available and waits for the user to choose **Refresh project instructions**.
- The refresh confirmation shows a compact source manifest rather than instruction contents: each applicable guidance file is identified by a cwd-relative path and `added`, `changed`, or `removed` status; skill-catalog changes are listed separately by skill name and status. Unchanged sources are summarized, not listed by default.
- The confirmation also shows an estimated one-time prompt-cache rewarm size in input tokens, labels it as an estimate, and explains that actual provider cache behavior may differ.
- Confirming refresh captures the exact normalized candidate bundle represented by that manifest. Later filesystem changes do not mutate the queued bundle; they remain visible as a newer refresh opportunity.
- If the agent is working, the captured bundle is queued. The current user turn and its complete tool loop finish under one bundle version. The queued version activates before the next user-authored turn.
- Conversation history is preserved across activation. Activation creates a visible transcript-generation boundary, invalidates incompatible continuation state, and records a visible timeline event.
- New conversations use the latest resolved project bundle.
- Path-scoped nested guidance discovered while accessing deeper files is separate work tracked by task 36012; this task must leave a composable bundle/generation contract for it.

## Acceptance criteria

- [ ] Guidance and skill-catalog snapshots are persisted as normalized schema, not hidden in an unrelated JSON blob.
- [ ] Every model request within a transcript generation uses the same persisted project bundle.
- [ ] Live runtime state remains structurally separate and authoritative without silently rewriting the project bundle.
- [ ] Source changes produce a visible stale/changed state but do not affect model requests until explicit refresh.
- [ ] Refresh identifies changed guidance sources by cwd-relative path and `added`, `changed`, or `removed` status without displaying file contents.
- [ ] Refresh identifies changed skill-catalog entries separately by skill name and status; unchanged sources remain collapsed or summarized.
- [ ] Refresh presents the source manifest plus an estimated one-time cache-rewarm token count before confirmation.
- [ ] Confirmation persists the exact candidate represented by the manifest, even if source files change again before queued activation.
- [ ] Refresh during active work queues activation until before the next user-authored turn; one tool loop cannot span project-bundle versions.
- [ ] Activation preserves history, creates a visible generation-boundary event, and clears incompatible provider continuation state.
- [ ] New conversations and pre-snapshot conversations have explicit, lossless initialization and recovery behavior.
- [ ] Tests mutate guidance, tasks, and skills between turns and prove the active bundle remains byte-identical until refresh.
- [ ] Tests cover source changes after confirmation but before queued activation.
- [ ] The dangling `TODO(task 61006)` is replaced with a durable local fact or normative spec reference.
- [ ] Cache-read measurements repeat the cold, warm, tool-loop, and mid-session guidance-mutation matrix before and after implementation.
- [ ] Specs describe bundle contents, stale detection, confirmation, queued activation, recovery, and generation boundaries.
- [ ] `./dev.py check` passes.
