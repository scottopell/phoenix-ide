# Iterate mobile conversation list fixtures with comprehensive conversation-state coverage

## Goal

Redesign what each conversation/chain entry communicates in the mobile conversation list so the interface is intentional, consistent, and never falls back to low-value identifiers like raw GUIDs. This task is explicitly collaborative: build a comprehensive fixture matrix, capture it, review screenshots with the user, iterate on the UI, and repeat until the mobile list has an agreed information hierarchy.

## Why

The current mobile conversation list can show inconsistent row structures and poor labels for some conversations/chains. The screenshot shows examples where dense metadata overlaps, chain entries compress awkwardly, and some items expose GUID-like labels instead of a meaningful title. The mobile surface needs a stable design contract for all realistic conversation states, not a one-off CSS fix for one screenshot.

## Scope

### 1. Enumerate conversation display states and combinations

Inventory the data that can affect a row or chain block, including at least:

- Standalone conversation vs. chain root/member/latest summary.
- Active, archived, terminal/done, idle/ready, working, error, awaiting approval, awaiting user response, context exhausted / continued.
- Managed modes: Explore, Work, Direct, Branch, and absent/unknown mode labels.
- PR badge variants: open, draft, merged, closed, no PR.
- Project/cwd context variants: project name, worktree path, cwd-only, long path, missing/unknown project.
- Naming variants: human slug, renamed chain, missing/poor slug, GUID-like slug, long slug, task title/branch fallback candidates if available.
- Recency/message-count edge cases where mobile layout must remain stable.

### 2. Expand the existing Ladle/mobile QA fixture

Use the existing mobile conversation list fixture and capture flow:

- `ui/src/fixtures/mobileConversationList/*`
- `ui/src/stories/mobile-conversation-list.stories.tsx`
- `./dev.py qa mobile-conversation-list`

Add scenario data that deliberately covers the inventory above. Prefer a small number of curated scenario screens over an unreviewable combinatorial explosion, but ensure every important display rule appears somewhere.

### 3. Define and iterate on the mobile row design contract

For each visible row/card, decide and implement a consistent hierarchy such as:

- Primary label: meaningful conversation/chain title, never raw GUID when a better label exists.
- Secondary context: mode/status/recency/project/PR in predictable positions.
- Chain behavior: clear distinction between chain title, latest member, historical members, and expandable/collapsed summaries.
- Mobile constraints: no overlapping text, stable truncation, accessible touch targets, and predictable badge wrapping.

Document the resulting display rules in tests or fixture naming where practical; avoid broad design prose unless it belongs in an existing spec.

### 4. Interactive review loop with the user

This task should proceed in short visual iterations:

1. Expand/adjust fixture data for coverage.
2. Run `./dev.py qa mobile-conversation-list`.
3. Share links/screenshots for review.
4. Ask the user what feels right/wrong.
5. Apply focused UI changes.
6. Repeat capture/review until accepted.

Do not treat the first implementation as final. The main deliverable is a user-reviewed, screenshot-backed mobile interface.

## Acceptance criteria

- The mobile conversation list Ladle fixture includes representative coverage for standalone conversations, chains, statuses, PR states, naming/path edge cases, and archived state.
- `./dev.py qa mobile-conversation-list` runs successfully and produces screenshots for review.
- Mobile rows do not overlap or expose GUID-like identifiers as the primary visible label when better semantic labels are available.
- Conversation and chain rows share an intentional, consistent information hierarchy across scenarios.
- At least one screenshot review loop with the user is completed before finalizing the design.
- Relevant component tests are updated or added for the display fallback rules and mobile chain/row rendering behavior.
- Final changes pass the project’s appropriate UI checks and `./dev.py check` if feasible for the scope.

## Notes

The existing QA command could not be run from Explore mode because the sandbox blocks dependency/temp writes under `ui/`. Run it from the approved writable task worktree.
