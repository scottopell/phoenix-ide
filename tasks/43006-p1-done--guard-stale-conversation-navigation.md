# Guard stale async navigation across conversation changes

ConversationPage has async slug-resolution continuations for fork outcomes and "Open work conversation" that navigate without rechecking page ownership. If a user moves from conversation A to B before the lookup resolves, A can later redirect the browser away from B. Bind these continuations to the initiating conversation/view and add A-to-B race regressions.
