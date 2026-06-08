The fork-resolution actor (ForkCommand consumer) shares a single tokio::select! task with the sub-agent spawn/cancel and task-handoff handlers in RuntimeManager::start_sub_agent_handler. Because each ForkCommand is awaited to completion before the loop takes the next event, a fork approve/promote git phase (worktree materialize + `git worktree add`, run in spawn_blocking but awaited in the loop) briefly blocks the spawn/cancel/handoff arms.

This is a minor latency coupling, not a correctness issue: fork resolution is human-gated and infrequent, the git phase is a few seconds, and there is no deadlock (no fork handler enqueues onto these channels). But it does mean a fork approval can delay a concurrent sub-agent spawn or task handoff.

Refinement: give fork-resolution commands their own dedicated consumer task (its own tokio::spawn loop) so fork ops serialize among themselves (preserving the correct-by-construction mutual exclusion) without coupling their latency to spawn/handoff. The git-name serialization with Explore approval (TASK_APPROVAL_MUTEX) is unaffected.

Surfaced during Codex review of the decoupled task-fork PR (#235).
