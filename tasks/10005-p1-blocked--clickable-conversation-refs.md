# Render Coordinator conversation references as clickable navigation

Plain durable conversation handles such as `@conv:<id>` currently remain inert text in Coordinator Markdown unless the agent separately emits a Markdown link. Make supported conversation handles a reliable clickable UI contract in Coordinator chat, preserving message targeting where a paired `msg:<id>` is present and carrying the Coordinator return origin used by the existing parent-style breadcrumb.

## Acceptance evidence

- A plain `@conv:<id>` in Coordinator agent prose renders as an app-local conversation link.
- A supported paired message reference navigates to the message fragment when present.
- The same prose outside Coordinator does not reinterpret ordinary `@` file/user text.
- Markdown links, code spans/fences, external links, and file references retain existing behavior.
- Mobile tapping navigates in the current context and the destination retains `← from: Coordinator`.
- Focused UI tests and `./dev.py check` pass.

## Blocked by lifecycle unification

Park until task 92009 and the work in production conversation `/c/conversation-lifecycle-workflow-clarification-4` land. That work introduces ProductConversation aggregate identity and replaces row-level lifecycle authority; implementing clickable `@conv` semantics before its reference/navigation contract settles would create migration and cleanup work.

After it lands, rebase from current `origin/main`, inspect the resulting canonical aggregate/member reference model, and implement clickable Coordinator references against that model rather than assuming a conversation row is the durable product identity.
