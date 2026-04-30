---
created: 2026-04-30
priority: p3
status: brainstorming
artifact: specs/chains/requirements.md
---

A user-curated, per-project text blob that Phoenix would inject into the system prompt of every conversation in that project. Sister concept to chains: chains recall PAST work, steering doc primes FUTURE work.

Originally pitched at the start of the chains design conversation as the "project summary" idea. Branch was originally named project-summary before being repurposed for chains. The chains spec lists this in the explicit non-requirements ("Project-level summary or steering doc. A separate concept, explicitly deferred.").

OPEN QUESTION worth resolving before any implementation: how is this meaningfully different from AGENTS.md? Initial framing was that AGENTS.md is committed to the repo (team-wide, stable conventions) while a steering doc is per-instance, personal, evolving, freely editable without git churn. BUT: AGENTS.md is just a markdown file, a user can locally edit it and Phoenix already picks up the changes via the existing discovery walk in src/system_prompt.rs. The "git churn" argument only bites if the user feels forced to commit their edits, which they are not.

Maintainer note (2026-04-30): hasn't felt the pain of repeating context across conversations enough to validate this as a distinct concept worth a new artifact, schema, or UI surface. Filed for future-self in case the pain shows up later.

Possible reframings if the idea is revived:
- Drop entirely if AGENTS.md ergonomically covers the use case
- Make it a UI affordance for editing AGENTS.md (cheap, no new concept, just a better editor)
- Make it ephemeral / per-session priming text -- a real distinction from AGENTS.md, but smaller scope than the original "project summary" idea

Trigger to revisit: maintainer notices repeated copying of context between conversations in the same project, or hears the same complaint from another user. Until then, do nothing.
