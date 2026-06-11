spawn_agents dispatch resets the sub-agent result buffer mid-round, destroying results buffered from an earlier batch in the same round.

handle_spawn_agents_tool begins with `self.sub_agent_result_buffer = Vec::with_capacity(input.tasks.len())` (executor.rs ~1302). The buffer holds SubAgentResult events that arrive while the parent is not yet AwaitingSubAgents (drained on entry ~895). With LLM tool sequence [spawn_agents A, bash, spawn_agents B] (supported by the SpawnAgentsComplete-more-tools arm, transition.rs ~969): while bash runs, agent A1 completes and is buffered; spawn_agents B reassigns the buffer and discards A1. Parent enters AwaitingSubAgents with A1 still pending, A1 never re-sends -> stall for the full DEFAULT_SUBAGENT_TIMEOUT (~20 min), then A1 real work replaced by synthetic TimedOut.

Fix: never reassign the buffer; reserve capacity instead, or scope the buffer lifetime to the awaiting-round rather than to a spawn call.

Found in spiritual-core audit 2026-06-10.
