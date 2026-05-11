---
created: 2026-05-10
priority: p2
status: ready
artifact: crates/phoenix-ide/src/runtime.rs
---

Steering queue may sit undrained after crash recovery in a narrow window. If a process crashes mid-drain (after partial ClearSteeringQueueEntries but before completion), or if state is loaded as a non-drain-hook state with a non-empty queue, the queue sits until a subsequent state transition triggers a drain hook (entering Idle or LlmRequesting from a tool round). If state is Error, AwaitingRecovery, AwaitingTaskApproval, AwaitingUserResponse, or ContextExhausted on resume, queued steers may wait indefinitely until user action.

Fix: add a startup-time drain trigger. On executor bootstrap, if context is parent and steering_queue is non-empty, evaluate the loaded state — if it accepts SteerDrainedUserMessages (Idle or LlmRequesting), fire the event immediately. If not, surface the queued steers in some UI affordance so the user knows they exist.

Related: specs/steering-messages/ — the inline-drain feature merged in PR #73 closed the race that motivated this concern but did not add startup drain.
