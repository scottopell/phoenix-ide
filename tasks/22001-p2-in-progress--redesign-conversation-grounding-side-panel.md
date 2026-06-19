# UX review and redesign conversation grounding side panel

## Goal

Transform the conversation side panel from an ad-hoc stack of Files, MCP, Skills, Tasks, and Work scope widgets into a visually consistent, reliable discovery surface that feels like the conversation's grounding/source-of-truth panel.

This is intentionally UX + QA first: build confidence with rich mock/seed data and screenshots before redesigning, then keep those scenarios as regression coverage.

## Current target surface

Primary code paths to audit and improve:

- `ui/src/components/FileExplorer/FileExplorerPanel.tsx`
  - Expanded and collapsed rail behavior
  - Detail-view replacement behavior for skills/tasks
- `ui/src/components/FileExplorer/FileTree.tsx`
- `ui/src/components/McpStatusPanel.tsx`
- `ui/src/components/SkillsPanel.tsx` and `SkillViewer.tsx`
- `ui/src/components/TasksPanel.tsx` and `TaskViewer.tsx`
- `ui/src/components/WorkScopePanel.tsx`
- Related CSS files for those panels plus shared layout in `ui/src/index.css`

## UX direction

The side panel should answer, at a glance:

1. What project/repository am I grounded in?
2. What files and recent/open files matter right now?
3. What capabilities are available to the agent? (MCP + skills)
4. What tasks/work items exist and which one is current?
5. What live runtime resources exist? (bash/tmux/browser work scope)
6. What needs attention? (auth failures, MCP failures, live processes, current task, unavailable resources)

Design principles:

- One coherent information architecture, not five unrelated widgets.
- Consistent section headers, counts, icons/glyphs, empty states, loading states, and error states.
- Dense but legible: status inline with values, progressive disclosure for details.
- Collapsed rail should remain useful, not cryptic (`/`, `T`, raw count badges need review).
- Preserve source-of-truth feel: status should be fresh, scoped to the active conversation, and visibly tied to the current cwd/worktree.
- Avoid duplicate information that competes for attention.
- Keyboard and screen-reader affordances should be reliable for all interactive rows/toggles.

## Required QA and screenshot setup

Before changing UX, create a repeatable QA fixture with full mock/seed data covering the whole panel. Prefer a declarative/component-level screenshot harness if feasible; otherwise use a deterministic seeded dev conversation and browser screenshots.

Evaluate options and implement the best practical one:

1. **Preferred if feasible:** component-level screenshot scenarios using Vitest/browser-mode, Playwright, Storybook, Ladle, or a similarly lightweight harness.
   - The repo currently has Vitest + Testing Library + happy-dom, but no screenshot system. If visual screenshots require adding a heavy framework, justify the tradeoff before doing so.
   - A lightweight local route/dev-only fixture component is acceptable if it keeps the scenarios deterministic and easy to capture.
2. **Fallback:** seed/mocking path for `./dev.py up` plus browser automation screenshots.
   - Use API mocks where possible for MCP/skills/tasks/workscope responses.
   - If backend seeding is needed, add a focused fixture path that does not pollute normal user data.

Screenshot scenarios must include at least:

- Expanded panel with all sections populated:
  - Deep file tree with selected/open file state and long path names.
  - MCP: ready enabled server, ready disabled server, unauthorized OAuth server with redirect warning, failed server with error, large tool list.
  - Skills: built-in, user, and project skills; long descriptions; argument hints; multiple groups.
  - Tasks: ready, in-progress/current, blocked, brainstorming, done, wont-do; linked conversation slug; long slugs; priorities p0-p4.
  - Work scope: running bash, kill-pending bash, successful tombstone, failed tombstone, live tmux, live/idle browser.
- Empty states for each section.
- Loading/error states for each async section.
- Collapsed rail with and without live/attention states.
- Narrow/resize states around the minimum useful panel width.
- Detail views for a selected skill and selected task, including long markdown content.
- Light and dark themes if the app supports both in this surface.

Store screenshots/artifacts in a discoverable QA location, or document the exact command that generates them. Avoid committing giant binary churn unless the chosen tooling is intentionally snapshot-based.

## Review plan

1. Inventory the current panel UX and interaction model.
   - Map each section's data source, loading lifecycle, refresh behavior, and active-conversation scoping.
   - Identify inconsistent controls, stale-data risks, visual hierarchy problems, and hidden/cryptic affordances.
2. Capture baseline screenshots for all required scenarios.
3. Produce a concise UX findings list before implementing changes.
   - Group findings by user impact: confusing grounding, unreliable state, inconsistent visuals, inefficient discovery, accessibility/keyboard issues.
4. Redesign the information architecture.
   - Decide whether this remains a stacked accordion, becomes grouped around “Project / Capabilities / Work”, or uses another consistent structure.
   - Define shared section/header/row primitives if useful.
   - Make attention states prominent but not noisy.
5. Implement the redesign incrementally.
   - Keep existing behavior working while changing presentation.
   - Preserve current task/skill detail affordances or replace them with a better consistent detail pattern.
   - Ensure async fetch cancellation and active-conversation scoping remain correct.
6. Re-capture screenshots and compare against baseline.
7. Add automated regression coverage.
   - Component/unit tests for section summaries, collapsed rail attention states, detail navigation, stale conversation scoping, and empty/error/loading states.
   - Screenshot/declarative fixtures if the chosen tooling supports it.
8. Run validation.
   - `./dev.py check` or the narrowest equivalent lanes required for UI changes.
   - Manual browser QA against the seeded/mock scenarios.

## Acceptance criteria

- The conversation side panel presents Files, MCP, Skills, Tasks, and Work scope as a coherent grounding panel with consistent visual language.
- Users can quickly determine current project grounding, available capabilities, active tasks, and live runtime resources.
- Attention states are visible in both expanded and collapsed modes.
- Empty/loading/error states are clear and consistent across sections.
- Rich screenshot scenarios exist and are documented/reproducible.
- Baseline and final screenshots are captured for review.
- Tests cover the most important behavior and scoping regressions.
- No stale data from a previous conversation is shown after navigation.
- The panel remains usable at narrow widths and through resize/collapse interactions.
- Keyboard interaction and basic accessibility are improved or at least not regressed.
