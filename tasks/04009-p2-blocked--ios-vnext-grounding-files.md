# Add iOS vNext grounding and file browsing

## Outcome

Satisfy `REQ-IOS-019` by letting the native client inspect ProductConversation grounding and browse server-hosted project files without pretending server paths are local phone paths.

## Dependencies

Blocked by ProductConversation migration and the rendering fixture harness.

## Scope

Catalog the authoritative grounding, task/skill, worktree, Git-status, and file-content surfaces, then create numbered `REQ-IOS-019` leaf tasks for the native panel and file browser. Every context root and file request remains bound to one exact attached `WorkScope`; portable file contents cross the API boundary, while server-host reveal/open actions are absent. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Every grounding root identifies its exact attached `WorkScope`; multiple roots remain separately selectable.
- The user can navigate supported server files and open their contents.
- Navigation, contents, and stale fallback remain bound to the selected scope and requested file identity.
- Phone-local and server-host desktop reveal/open actions are not offered.
- Loading, empty, stale, offline, permission, missing-file, and error states are intentional.

## Out of scope

Editing files, terminal access, and prose commenting.
