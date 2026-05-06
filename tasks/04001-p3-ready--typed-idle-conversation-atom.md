---
created: 2026-05-06
priority: p3
status: ready
artifact: ui/src/conversation/useConversationAtom.ts
---

Introduce a typed IdleConversationAtom variant (or discriminated return from useConversationAtom) so that the continuation callback is only a property of the idle branch. Currently, onTriggerContinuation is present/absent via a runtime check on convState.type === "idle" in StateBar, which creates a race window where the button can appear but the action can be silently blocked. A structural fix would make the callback unconditionally present on the idle type — no runtime guard needed anywhere because the type enforces it. This eliminates the class of "button visible, action blocked" bugs across any future features that gate on phase.
