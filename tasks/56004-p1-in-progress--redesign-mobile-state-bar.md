# Redesign mobile conversation state bar

## Problem

The mobile StateBar rendered under the conversation composer is too dense. Its collapsed mode hides some useful controls, while expanded mode mostly compresses the desktop two-line layout into a narrow viewport. The redesign must start from the mobile use case rather than tweaking desktop CSS.

## Mobile information triage

The current StateBar derives many fields, but mobile should not treat every field as equally important. Preserve access to the useful information while intentionally dropping low-value clutter from the mobile presentation.

### Must be first-class on mobile

- Navigation identity: back-to-conversations affordance and conversation slug.
- Current working directory: show a readable cwd/path summary and provide an explicit copyable full cwd value.
- Conversation mode:
  - `Explore` with explicit read-only meaning.
  - `Work` task-branch meaning.
  - `Branch` existing-branch meaning.
  - `Direct` / fallback full-access meaning.
- Model identity: abbreviated model in compact areas, full model in title/picker/detail, interactive model picker only when `canChangeModelInState(convState)` permits it.
- Conversation/work identity:
  - Work mode task title when present.
  - Branch name when present.
  - PR status badge or unavailable hint when branch PR status is known.
- Context window indicator when usage exists, including existing warning/critical styling and continuation trigger availability only while idle.
- Mobile file-browser button (`onOpenFiles`) with accessible target size and no accidental expand/collapse activation.
- Mobile expand/collapse state and keyboard accessibility.

### Can be minimized to save space

- Live state/connection information can be represented compactly when space is tight, as long as important exceptional states remain visible:
  - Ready/completed/handoff/approval/user-reply/error/context-full.
  - Working phase elapsed time and streaming state.
  - Retry suffix when present.
  - Heartbeat degraded state.
  - Reconnecting/offline states, including frozen last-known activity when present.
  - Offline reconnect countdown banner and Retry now action.

### Drop from mobile unless needed for a specific control

- Base branch. It is not important enough for the mobile StateBar and should not consume space.
- Project name. Cwd is the more useful mobile identity; project name should not be shown as a separate mobile datum.

Desktop behavior may keep existing base-branch/project-name affordances unless simplifying shared rendering makes a deliberate desktop change desirable.

## Proposed layout direction

Replace the current mobile adaptation of the desktop layout with a dedicated mobile information hierarchy:

1. **Collapsed mobile bar: one scannable primary row**
   - Left: back affordance + concise conversation identity.
   - Middle/right: compact status indicator that can collapse routine states to a dot/short label.
   - Actions: files button and expand control remain reachable.
   - Avoid competing inline chips. Mode/model/git/context should not all fight for the collapsed row.

2. **Expanded mobile sheet/stack: grouped details**
   - Group path: cwd summary plus copy-full-cwd control.
   - Group identity: mode, model picker, task title when present.
   - Group branch/PR: branch name and PR badge/hint, without base branch.
   - Group runtime/context: compact runtime state, connection/retry/last-known activity when relevant, context indicator.
   - Keep tap targets >= 44px where controls are interactive.
   - Avoid horizontal overflow; long slugs, cwd paths, task titles, branches, and model ids must truncate, wrap, or copy deliberately.

3. **Desktop/tablet behavior**
   - Preserve the current desktop layout unless a change is necessary to share clean rendering helpers.
   - Keep existing model picker, PR badge, context menu, and offline banner behavior.

## Implementation plan

1. Audit `StateBar.tsx` and factor current derived display values into small typed render helpers so desktop and mobile cannot drift on what data exists.
2. Introduce a dedicated mobile render branch below the existing `useIsMobile()` decision instead of relying primarily on CSS hiding.
3. Define explicit mobile sections for primary, cwd/copy, identity, branch/PR, model, runtime, and context/action groups.
4. Update `ui/src/index.css` mobile StateBar rules from hide/show overrides to styles for the new mobile structure.
5. Add/adjust tests in `StateBar.test.tsx` for:
   - Explore conversation: mode/read-only, model, copyable cwd, compact runtime state all accessible on mobile.
   - Work conversation: task title, branch, PR badge/hint, model, context, copyable cwd, files action all accessible on mobile; base branch not shown in the mobile layout.
   - Branch conversation: branch info preserved without task title or base branch.
   - Direct/fallback conversation: direct/full-access mode and copyable cwd preserved; project name not shown as separate mobile identity.
   - Collapsed vs expanded mobile behavior: collapsed is sparse, expanded exposes the first-class details.
   - Existing working-phase, retry, heartbeat, reconnect/offline, model-picker enablement, context continuation, and file-button keyboard behavior continue to pass.
6. Run the targeted UI tests and the project check lane appropriate for UI changes.

## Acceptance criteria

- On <=768px viewports, the collapsed StateBar is visually sparse and scannable.
- Expanding the StateBar exposes the first-class mobile information above, especially a copyable cwd, without relying on hidden desktop-only content.
- Base branch and project name are not shown in the mobile layout.
- All conversation modes (`Explore`, `Work`, `Branch`, `Direct`/fallback) have an intentional mobile layout.
- Routine runtime state may be compact, but exceptional runtime/connection states remain visible enough to act on.
- No regression to desktop StateBar behavior.
- StateBar tests cover mobile layout for all conversation modes and critical status/connection/context affordances.
