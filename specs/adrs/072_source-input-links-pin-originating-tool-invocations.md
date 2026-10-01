# ADR-072: Source input links pin originating tool invocations

- **Status:** Accepted
- **Date:** 2026-10-01
- **Affects:** REQ-GR-014, REQ-COMP-001

## Context

Conversation provenance identified the sender transcript but could not identify the send invocation. Global transcripts failed ordinary product snapshot routing. A transcript-only link could not satisfy one-click source inspection.

## Options considered

- Infer the call from matching body text or recipient identifiers: ambiguous and not source authority.
- Store only a tool identifier: insufficient to retrieve a virtualized historical message directly.
- Store a server-bound message/tool pair alongside exact transcript identity: chosen.

## Decision

Tool execution supplies the owning assistant message and tool-use identifier. Internal input carries an optional typed pair, stored as paired nullable columns in message, steering, and durable-turn rows. Migration 112 adds the columns without fabricating old locators. Historical absence remains unavailable, including decoded pre-feature admission payloads. No downgrade or mixed-version runtime guarantee is added.

Web navigation classifies Global before ordinary product snapshots, pins the historical member, and uses its message anchor plus source-tool navigation parameter to expand and highlight the invocation. These browser coordinates do not extend the model-facing conversation-reference grammar. Native decoding retains the pair without claiming native source-jump UI coverage.

## Consequences

Admission/retrieval and normal/provider-replay persistence require round-trip regressions. Global and ordinary browser click journeys must cover virtualized/collapsed tools. Missing source members remain errors in the normal layout; old input does not become user-authored or acquire guessed anchors.
