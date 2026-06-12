Implement sandboxed bash for Explore sub-agents so top-level Explore can safely expose spawn_agents again.

Context: PR #270 removed spawn_agents from top-level Explore registries because Explore sub-agents still receive the unrestricted BashTool via the sub-agent registry path. That avoids the sandbox bypass but removes useful parallel exploration. The proper fix is to make Explore sub-agent bash use the same OS-enforced read-only policy as top-level Explore before re-enabling parent Explore spawning.

Acceptance criteria:
- Explore sub-agent bash uses the nono-backed read-only sandbox, not unrestricted BashTool.
- A top-level Explore parent can expose spawn_agents only when spawned Explore sub-agents are sandboxed on the current host.
- Explore sub-agent bash can perform broad local reads and Git/status investigation.
- Explore sub-agent bash cannot mutate source/Git metadata or use network.
- Scratch/home/temp/env policy matches top-level Explore semantics unless explicitly documented otherwise.
- Work/Direct/Branch sub-agent behavior remains unchanged.
- Specs for subagents/projects/bash are updated to describe the restored Explore spawning path and sandbox guarantees.
