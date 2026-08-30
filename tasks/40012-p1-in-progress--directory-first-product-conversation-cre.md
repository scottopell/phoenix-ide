# Directory-first ProductConversation creation

Parent: task 92009 (`unify-conversation-workstream-lifecycle`).

Replace the normal web New Conversation journey with a directory-first typed ProductConversation creation path. The request accepts directory plus objective/model/settings only; Project identity, conversation mode, base branch, branch name, checkout ref, and early-worktree promotion taxonomy are structurally absent. The server derives Git-backed versus non-Git creation and uses the existing durable creation protocol as the sole production writer. Git-backed creation provisions a fresh ProductConversation/root with a Phoenix-owned detached-default WorkScope/worktree; non-Git creation is Direct. Navigate only to the canonical ProductConversation route after durable acceptance, with truthful loading, retry, cancellation, and reload/recovery behavior.

Recent directories may remain suggestions but are not durable Project identity. Remove Project settings/tabs from this creation journey and update timeless specifications/tests where they already declare Project-era creation retired. The legacy endpoint may remain unreachable compatibility until task 92016.

Out of scope: Close and member-lifecycle behavior; Project grouping beyond removing creation inputs; durable-engine cutover task 40010 and its substrate.

Acceptance gates: focused Rust and TypeScript tests plus browser journeys cover Git and non-Git directories, no legacy inputs, independent Git worktrees, canonical navigation, and reload/recovery; full `./dev.py check`; immutable adversarial review; exact-head Codex/CI; zero unresolved PR threads; human merge gate.
