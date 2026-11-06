# Feature Ember consistently across conversation lists

## Approved design direction

The user requested a conversation-list redesign that features Ember and consistently uses Ember as the name, while retaining “Global coordinator” as a role description, on mobile and desktop. They selected **Pinned compact entry** and **Web and native iOS**.

Implement a compact pinned entry above the ordinary list controls:

```text
Phoenix                         + New

[mark] Ember                       →
       Global coordinator

Open 12        History 38
────────────────────────────────────
Conversation…
```

Phoenix remains the application brand. Ember is the collaborator's name, not a replacement app name. Use restrained accent styling and a consistent recognizable mark, not a large hero, mascot illustration or lore-heavy copy. A static mark must not look like a live activity indicator.

## Observed journey and environment

Read-only investigation in worktree `9f21dd60-57bc-4cd5-8ca9-f4bc7f11b415`. Web desktop exposes an icon-only Coordinator action in the sidebar; mobile web exposes an icon-only Coordinator action in its list header. Native iOS exposes a globe toolbar action. Opening those actions uses the existing singleton Coordinator navigation. This investigation is source-grounded; no rendered-layout validation has yet been performed. GitHub roadmap #806 could not be verified because network access was blocked.

## Verified findings

- `ui/src/components/Sidebar.tsx` has separate expanded and collapsed Coordinator actions, both labelled Coordinator and navigating to `/global`.
- `ui/src/components/ConversationList.tsx` renders the mobile header's “Open Coordinator” icon button. `ConversationListPage.tsx` renders `NewConversationPage` on desktop, making the sidebar the desktop list owner. Its loading branch has a separate header.
- `CoordinatorPage.tsx` contains Coordinator loading/accessibility copy; `ConversationReturnBreadcrumb.tsx` renders “from: Coordinator”. Follow destination header identity through `ProductConversationPage` before editing presentation.
- `ios/PhoenixMobile/Sources/Views/ConversationListView.swift` uses `model.openCoordinator()`, a repeated-open guard, and offline availability gating. Its row presentation names Coordinator explicitly.
- ADR-214 establishes the Ember persona but expressly leaves UI branding undecided.
- REQ-GR-006 separates the singleton from ordinary Open/History lifecycle and Close/Delete controls. Preserve this distinction.

## Interaction map and invariant

List entry → existing web `/global` route or native `openCoordinator()` → existing durable singleton identity → normal chat experience. Navigation is a presentation change, not creation of another kind of conversation or another identity.

Ember must be identifiable and reachable independently of list filtering, ordinary conversation count, loading and list scroll position. Loading/failure of the ordinary list must not unnecessarily hide the navigation entry. Preserve existing platform offline behavior rather than introducing a new offline promise.

## Implementation scope

1. **Desktop web:** place a named Ember entry below the application toolbar and above Open/History and scoped list content. Keep it pinned outside the list scroller. Remove the redundant old Coordinator utility icon. Preserve New, terminal, settings, local services and sidebar resize/collapse functionality. The collapsed rail retains one Ember entry, with a recognizable mark, tooltip and accessible name. Show selected state when viewing Ember, not an ordinary conversation.
2. **Mobile web:** use the same compact name/role hierarchy below the app toolbar and above Open/History. Keep it reachable while the list scrolls, without breaking the existing scroll owner, restoration or responsive header behavior. Remove the duplicate Coordinator icon. Keep New prominent and preserve utilities/auth feedback. Include initial loading and empty/History states; avoid sticky-header space consuming most of a short viewport.
3. **Native iOS:** replace the globe-only toolbar entry with the equivalent pinned compact Ember entry above ordinary list controls. Reuse `openCoordinator()`, its in-flight guard and offline-cache gating. Preserve navigation, list refresh, settings, New, confirmation dialogs, Dynamic Type and safe areas. Give disabled/offline/loading states truthful, accessible feedback.
4. **Branding along the journey:** use Ember as the primary visible name on the entry, destination header/title, loading and accessibility copy, and return breadcrumbs. Retain Global coordinator as secondary role description. Audit other directly related user-facing naming on these three surfaces to avoid opening Ember and landing on a differently named collaborator. Do not mechanically rename internal Coordinator types or technical role descriptions, remote provenance, stored user content or historical transcripts.
5. **Documentation:** add the named-entry and branding presentation contract to the owning normative web/global and iOS requirements; update relevant executive summaries and record the UI branding/layout decision in a new ADR. Follow `specs/AUTHORING.md`; do not rewrite ADR-214. No new Allium spec is needed for this presentation change.
6. Colocate component/page-specific CSS with its owner; use a shared web entry component if it removes real desktop/mobile branding duplication. Do not create a new design-system framework or backend brand service.

## Validation and acceptance evidence

- Web component/navigation tests: one Ember entry per layout; accessible name “Open Ember” (or equivalently clear link name); correct existing route; selection on Ember routes; no duplicate Coordinator utility action; collapsed rail remains usable by keyboard.
- Exercise Open/History, project/scope filtering, empty and loading lists, cached-list failure, long scrolling, list return/scroll restoration and desktop resize/collapse. Ember remains separate from counts and ordinary lifecycle actions.
- Native tests cover correct navigation, prevention of duplicate opens, cached offline availability and unavailable offline state. Preserve existing model behavior; do not replace it with independent UI state rules.
- Render desktop expanded/collapsed, narrow mobile web (including 320px), short-height/landscape and native iOS at normal and large text sizes. Check light/dark themes, visible keyboard focus, VoiceOver/screen-reader naming and touch targets. No horizontal overflow or inaccessible row actions.
- Update existing mobile list/Ladle fixtures and relevant QA captures to exercise the real entry; collect before/after screenshots for desktop, mobile web and native iOS where tooling is available. Explicitly report any native simulator/build verification that cannot be run.
- Run relevant targeted tests and `./dev.py check` using repository workflow. Read roadmap #806 if accessible in Work mode.

## Risks and explicit non-goals

Pinned content can interfere with scroll ownership, restoration, narrow sidebar space, and Dynamic Type. Resolve layout structurally instead of accumulating overlapping sticky offsets. Avoid duplicate Ember navigation between shared list content and its shell.

No Ember-first default home, large greeting card, composer embedded in the list, invented live summary, new unread/activity system, backend/API/schema/permission change, identity migration, route rename, prompt/persona rewrite, global terminology replacement, or remote Coordinator rebranding is included. Existing singleton continuation and persistence remain untouched.

## Task allocation

This is a plain proposal brief because Explore mode cannot run the repository's write-capable `taskmd new` allocator. After approval, use `taskmd new` to allocate the tracked implementation task rather than hand-allocating a numeric ID.
