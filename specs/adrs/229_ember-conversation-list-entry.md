# ADR-229: Name and pin Ember in conversation navigation

- **Status:** Accepted
- **Date:** 2026-11-05
- **Affects:** `specs/global-recall/requirements.md` REQ-GR-023; `specs/ios_client/requirements.md` REQ-IOS-019

## Context

The singleton Global Coordinator is a distinct collaborator, but web list navigation and the native list toolbar presented it as an icon-only Coordinator action. The user-facing identity should be consistent with the established Ember persona while preserving the technical role description and ordinary conversation lifecycle boundary.

## Options considered

1. Keep icon-only Coordinator controls and rely on the persona prompt; this leaves the name undiscoverable and inconsistent with Ember's role.
2. Replace every internal and technical Coordinator term with Ember; this conflates a persona with the stable role and API terminology.
3. Add one compact named Ember navigation entry and update directly related presentation copy while retaining internal Coordinator naming and singleton behavior.

## Decision

Present a compact named Ember entry, with “Global coordinator” as its secondary role, independently of ordinary Open/History rows and counts. Keep the desktop entry pinned in the sidebar and preserve a compact accessible mark in the collapsed rail; expose the same identity in mobile web list navigation and native iOS access. Use Ember as the primary presentation identity along the navigation journey. Retain the existing route, singleton identity, native open guard, and offline gating.

## Consequences

Ember becomes the visible navigation name without renaming internal Coordinator concepts or changing persistence, routing, permissions, or lifecycle behavior. The entry must not be presented as an ordinary conversation or imply live activity.
