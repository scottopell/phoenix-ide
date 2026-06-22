# Preserve grounding panel position after viewing tasks and skills

## Problem

Opening a task or skill from the Grounding side panel replaces the panel list with a detail viewer. Pressing the viewer Back button returns to the default panel composition, but the user loses their place: Tasks collapses back to its default and any in-list expansion/scroll context is gone.

## Evidence

`FileExplorerPanel` owns `selectedSkill` / `selectedTask` and swaps between `detailViewer` and the panel list. When a task is selected, `TasksPanel` unmounts. `TasksPanel` owns its own `expanded`, `groupExpanded`, loaded task list, and scroll DOM, so Back remounts it from defaults. Skills already has panel expanded state lifted to `FileExplorerPanel`, but its group/scroll state still lives inside `SkillsPanel` and is lost on unmount.

Key files:

- `ui/src/components/FileExplorer/FileExplorerPanel.tsx`
- `ui/src/components/TasksPanel.tsx`
- `ui/src/components/SkillsPanel.tsx`

## Desired behavior

When I open a task or skill from the side panel and click Back:

- Return to the same grounding section I came from.
- Keep that section expanded.
- Preserve group open/closed state.
- Preserve scroll position well enough that the clicked item is still nearby/in view.
- Do not affect behavior when changing conversations/projects.

## Implementation plan

1. Lift Tasks panel state into `FileExplorerPanel` like Skills already partially does:
   - `tasksPanelExpanded`
   - task `groupExpanded`
   - optionally the loaded `tasks` list if needed to avoid a reload flicker on Back.
2. Make `TasksPanel` support controlled expansion/group state props while retaining internal defaults when used elsewhere.
3. Make `SkillsPanel` support controlled group state and scroll restoration, or otherwise keep it mounted while the detail viewer is shown.
4. Preserve scroll position for the panel list before switching to `SkillViewer` / `TaskViewer`; restore it on Back after the list remounts.
5. Add RTL tests:
   - expand Tasks, open a non-default group, click a task, Back → Tasks still expanded and group state preserved.
   - expand Skills, toggle a group, open a skill, Back → Skills still expanded and group state preserved.
   - conversation change resets task/skill list state so stale context does not leak.
6. Run targeted UI tests, then `./dev.py check`.

## Notes

Prefer state lifting over localStorage. This is per mounted grounding panel context, not a durable user preference.
