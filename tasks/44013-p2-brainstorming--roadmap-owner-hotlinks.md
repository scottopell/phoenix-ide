# Coding-harness owner identity and optional verified hotlinks

## Need

Delivery coordination identifies owners with human-readable labels, but a project manager cannot reliably navigate to the owning coding-agent conversation. The retired roadmap is not a destination for this work; its former hotlink proposal is historical context. Capture a stable harness-native identity and, when the harness exposes a verified deep-link contract, an optional hotlink.

## Facts established

- A human title or slug is mutable and is not identity.
- Codex exposes an opaque task ID and finer turn/item identifiers through its task-management capabilities.
- No public Codex deep-link contract was verified. A `codex://task/...` shape is only a proposal and must not be treated as supported.
- Phoenix supports web conversation routes such as `/c/<slug-or-id>` in this checkout.
- The macOS Phoenix app's custom URL scheme is not available in this checkout, so neither `phoenix://c/...` nor `phoenix://conversation/...` is approved.

## Required distinctions

Any future design must keep these semantic values separate:

- stable opaque harness identity;
- mutable human-readable owner label;
- optional verified hotlink capability;
- optional turn/item anchor within the owning conversation.

A hotlink must not become the identity, and an invented URI must not represent an unavailable capability.

## First user journey

```text
Project manager reads a current task/PR or coordination handoff
  -> sees which coding harness/conversation owns it
  -> opens the owning conversation when a verified hotlink exists
  -> otherwise can copy the stable opaque identity
```

## Discovery before design

1. Verify and document the Phoenix macOS app's actual custom URL contract from its owning implementation.
2. Determine whether Codex exposes a supported external navigation API or URI contract; do not infer one from its internal task-navigation primitive.
3. Decide which existing task/PR or coordination surface needs a generic owner reference or verified hotlink, while preserving the distinctions above; do not revive the retired roadmap.
4. Define behavior for unavailable harnesses and anchors without building presence, messaging, an alias registry, or a general agent directory.

## Out of scope

- Choosing a URI scheme in this task.
- Implementing a cross-harness adapter registry.
- Agent messaging or presence.
- Treating project, host, working directory, title, or slug as conversation identity.
