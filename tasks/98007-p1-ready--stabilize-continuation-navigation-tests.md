# Stabilize continuation navigation integration tests

The two existing-continuation cases in `ConversationPage.archived.test.tsx` pass alone but repeatedly fail under the full CI suite because shared router/API mock state races the continuation-link click. Re-enable them after isolating their navigation harness from suite-global mocks. The core dispatch-failed edited-handoff case remains enabled.
