# Apply path-scoped AGENTS.md guidance when agents access nested files

Phoenix discovers AGENTS.md/AGENT.md only from the conversation cwd toward the filesystem root when constructing the system prompt. It does not discover deeper guidance that governs a file when an agent reads, edits, searches, or otherwise acts on a nested path—for example, accessing `a/b/c/d.txt` should make applicable `a/b/c/AGENTS.md` guidance available before the action.

This needs a deliberate cross-tool contract rather than ad hoc prompt mutation. Define which path-bearing operations trigger discovery, how multiple paths and directory operations resolve scope and precedence, when guidance must be delivered relative to a potentially destructive action, how symlinks/worktree boundaries behave, and how newly discovered guidance interacts with conversation prompt snapshots and cache stability. Preserve conversation continuity and do not silently claim compliance when guidance was discovered only after an edit.

Acceptance criteria:
- [ ] Requirements define applicability and precedence for root and nested AGENTS.md/AGENT.md files.
- [ ] Every path-bearing file operation either resolves applicable guidance before execution or is structurally outside the guidance contract.
- [ ] Multi-path operations, recursive search/listing, symlinks, missing paths, and paths outside cwd have explicit behavior.
- [ ] Nested guidance is delivered to the model before a governed mutation, not merely reported afterward.
- [ ] The design composes explicitly with system-prompt snapshot generations and manual instruction refresh.
- [ ] Discovery and delivery are deduplicated without making changed guidance silently live mid-generation.
- [ ] Regression tests cover read, search, patch/write, and shell-mediated file access or document any capability boundary that cannot be enforced.
- [ ] `./dev.py check` passes.
