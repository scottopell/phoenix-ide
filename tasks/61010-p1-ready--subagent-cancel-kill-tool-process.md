A cancelled Work-mode sub-agent still leaks a running bash process that keeps mutating the worktree it SHARES with its parent. Codex flagged this (PR #265 r5); the ordering half was fixed (sub-agent cancel now routes through CancellingTool and only notifies the parent after the in-flight tool task settles), but the bash PROCESS is not killed — bash deliberately detaches on cancel ("cancel != kill", normative in specs/bash). So the detached process (and any older backgrounded handles the sub-agent minted) keep writing to the shared worktree after the parent resumes.

Why the full fix was deferred:
- A Work sub-agent shares its parent WorkScope + handle table (runtime.rs ~1321, handle_spawn_request ~1614). A scope-wide kill (cascade_bash_on_delete style) would SIGKILL the PARENT bash too — worse bug.
- Handle (handle.rs ~185) carries NO owning-conversation attribution, and there is no tool_use_id -> handle_id map, so "this sub-agents handles" cannot be identified today.

Proposed design (precise, parent-safe, keeps cancel-Effect purity):
1. Add owner_conversation_id: String to Handle, set at spawn from ctx.conversation_id (registry spawn path).
2. Add Effect::KillTool (or extend AbortTool with kill: bool). The sub-agent ToolExecuting+UserCancel arm emits the kill variant.
3. New executor handler: enumerate the shared scope handle table, filter to owner_conversation_id == sub_agent_id, SIGKILL only those pgids via send_signal_to_group, before the tool settles. Never touches parent handles.
4. Update specs/bash (bash.allium / design.md): cancel still never kills for the PARENT/normal case; sub-agent TEARDOWN forces a kill of that sub-agents own handles. Keep the parent detach-dont-kill invariant; carve out sub-agent teardown.
5. Cap-accounting / handle-display touch-ups for the new field.

Preserve: reducer purity (kill is an Effect), tool_use/tool_result pairing, sub-agent fan-in conservation. Parent cancel semantics unchanged.

Anchors: transition.rs sub-agent CancellingTool arms (landed this round); executor.rs Effect::AbortTool handler (~2125, token-cancel only); operations.rs ~796 (cancel=still_running), run_kill ~868 / send_signal_to_group; registry.rs cascade_bash_on_delete ~447.
