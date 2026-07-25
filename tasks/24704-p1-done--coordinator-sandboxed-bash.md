# Reuse Explore-sandboxed Bash for Coordinator

Reuse the existing Explore-mode nono-sandboxed Bash capability for Coordinator repository and filesystem investigation. Expose authoritative conversation/WorkScope cwd and worktree paths in Coordinator read context, allow an explicit per-call cwd validated against persisted WorkScope environments, and do not introduce a separate repository-inspection command language or authority profile.
