# Add iOS vNext grounding and file browsing

## Outcome

Satisfy `REQ-IOS-019` by letting the native client inspect ProductConversation grounding and browse server-hosted project files without pretending server paths are local phone paths.

## Dependencies

Blocked by ProductConversation migration and the rendering fixture harness.

## Scope

Catalog the authoritative grounding, task/skill, worktree, Git-status, and file-content surfaces, then create numbered `REQ-IOS-019` leaf tasks for the native panel and file browser. File locations remain server-side handles; portable file contents must cross the API boundary. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Grounding is scoped to the ProductConversation and its attached environment.
- The user can navigate supported server files and open their contents.
- Remote/server-local actions are not presented as phone-local filesystem actions.
- Loading, empty, stale, offline, permission, missing-file, and error states are intentional.

## Out of scope

Editing files, terminal access, and prose commenting.
