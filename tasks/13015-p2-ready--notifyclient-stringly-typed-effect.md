Effect::NotifyClient { event_type: String, data: serde_json::Value } is a stringly-typed effect violating correct-by-construction, while every other Effect variant is precisely typed.

Location: crates/phoenix-ide/src/state_machine/effect.rs:141 (constructors ~288-315; consumer runtime/executor.rs:1362-1380; re-matched on literals at transition.rs:2903/3389)

There are exactly three legal event_type values ("state_change", "agent_done", "message"). An unknown/typoed value silently hits the `_ => {}` arm in the executor -- no log, no error, the notification just vanishes (also violates "capability gaps are logged, not silenced"). data: Value can carry any shape regardless of event_type and is dead for state_change (#[allow(dead_code)], with an existing comment admitting it should be replaced with typed effect variants). The STATE_CHANGE_EVENT_TYPE const (effect.rs:85) is a half-measure acknowledging the smell without fixing the structure.

Fix direction: replace with three typed variants (NotifyStateChange / NotifyAgentDone / NotifyMessage) carrying their own payloads, making the unreachable _ => {} arm structurally impossible and removing the dead data: Value field.
