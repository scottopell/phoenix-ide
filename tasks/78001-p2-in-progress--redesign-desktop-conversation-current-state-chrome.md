# Redesign desktop conversation current-state chrome

## Design brief

### Goal

At 1440×900, a user should be able to answer these questions without decoding paths, UUIDs, badge jargon, arrows, or duplicated labels:

1. Which conversation am I in?
2. Which project/worktree is it operating in?
3. Which mode and model are active?
4. Which branch is checked out, and what is its base branch?
5. Which associated pull request is active/open, and are there additional associated PRs?
6. Is the conversation ready, working, blocked, or waiting for me?

The desktop chrome should present one coherent current-state hierarchy rather than making the user assemble the answer from the sidebar, grounding header, bottom StateBar, work-actions rail, tooltips, and URL.

## Observed journey

Browser review performed against the live dev instance at `https://localhost:8032`, at 1440×900, without Ladle.

1. Opened `/new` and reviewed the desktop new-conversation card and persistent sidebar.
2. Created a fresh conversation named from the prompt: `desktop-conversation-chrome-review`.
3. Reviewed the new Direct conversation while it was working and after it accumulated activity.
4. Opened the selected conversation's action menu.
5. Compared seeded Direct, Branch (`fixture-diff-review`), and Work (`fixture-grounding-panel-qa`) conversations.
6. Collapsed the grounding panel and reviewed the remaining persistent chrome.
7. Opened a dense 484-message Direct conversation to see the chrome under realistic navigation pressure.
8. Checked visible browser console output; no chrome-specific runtime error was needed to reproduce these findings.

## Verified findings

### 1. An opaque worktree UUID is presented as a project name

The sidebar project tab displayed `eaed0971-83fd-42a2-94b0-a0d70fea99c2`, and Direct-mode StateBar line 2 displayed the same UUID. It also appeared in the dense conversation's grounding/path context. This is implementation identity, not useful user identity.

The UI already recognizes this bug class inconsistently: `ConversationList.displayTitleFromConversation()` rejects UUID-like conversation titles, while `ConversationList.compactContextLabel()` returns `project_name` or the path leaf without applying that rejection. `StateBar.getProjectName()` suppresses UUIDs only when inferred from a `.phoenix/worktrees/` cwd, but trusts an explicit UUID-valued `project_name`.

### 2. Conversation identity is fragmented and duplicated

The focused conversation name appears in the selected sidebar row and again in the bottom StateBar. Project/worktree/branch identity appears in both the grounding header and the StateBar, using different derivation and fallback rules. The route may additionally expose an opaque conversation UUID. No single region reads as an intentional “current conversation” summary.

### 3. Work and Branch context is information-rich but unlabeled and hard to parse

The Work StateBar compressed these values into two lines:

- conversation slug
- `Work` badge
- model
- task title
- `main ← task-22001-…`
- `PR status unavailable`
- a detached right-aligned `ready`

The Branch fixture similarly showed `main ← seed-diff-review`. The arrow does not communicate whether it means “branched from,” “merge into,” or data flow. Most meaning is available only via hover titles.

### 4. PR state is technically present but not a useful current-state answer

In the reviewed fixtures the only visible result was `PR status unavailable`, styled at the same hierarchy as branch identity. The focused chrome did not answer “is there an open associated PR?” in plain language. Existing code/specs already support an active PR badge/selector and distinguish cached sidebar summaries from live focused-conversation status; the redesign should improve presentation without replacing that authority model.

### 5. Activity state has the correct owner but weak visual relationship to identity

The right side of the StateBar correctly owns `ready` / working state, consistent with `REQ-CONV-007`. However, it appears detached from the identity cluster, making the footer read like unrelated fragments rather than one status summary. This task must preserve the StateBar as the sole live-activity owner.

### 6. The grounding header repeats branch context and truncates it

`FileExplorerPanel` derives its own project label from the root-path leaf and renders `project · branch` under `GROUNDING`. Long branches truncate almost immediately in the narrow pane. Branch is already shown in the StateBar, so the repetition adds noise while still failing to expose the full value.

### 7. Sidebar rows prioritize mechanics over resumability

Rows show a truncated slug, a mode badge such as `DIRECT`, timestamp range, message count, and state dot. The project tabs above them may themselves be UUIDs. The result supports scanning activity but provides weak human context for similarly named conversations and does not consistently satisfy the spirit of `REQ-CONV-001`'s working-directory context.

### 8. Labels are inconsistent in case and meaning

The same mode appears as uppercase `DIRECT` in the sidebar and title case `Direct` in the StateBar. `Work`, `Branch`, `Direct`, task title, project, worktree, base branch, branch, and PR status are visually adjacent without stable labels or grouping. `Grounding` is product terminology, while its collapsed tab says `Files`; neither clarifies whether branch/worktree identity belongs there.

## Information architecture proposal

### Canonical focused-conversation StateBar

Keep the StateBar at the bottom and preserve its ownership of live activity, but treat it as one semantic current-state component with three zones:

**Identity (left)**
- Human conversation name as the primary value.
- Project/repository as secondary context, using a human name such as `phoenix-ide`, never a UUID.
- Keep “back to conversations” as a separate icon/button affordance rather than embedding its arrow into the name.

**Work context (center, only when applicable)**
- Mode and model as compact, consistently cased chips.
- Explicit branch language: `Branch task-22001-…` and `from main`, not `main ← branch`.
- For Work mode, show the human task title before the mechanical branch name.
- Show worktree path only on demand (tooltip/details/copy), summarized from a meaningful repository-relative or leaf context; never promote `.phoenix/worktrees/<uuid>` as identity.
- Show the active associated PR as a semantic link, for example `PR #123 · Open`. If multiple associated candidates exist, show the active one plus a selector/count such as `+2`; do not invent repository-wide PR discovery or silently choose a different active PR.
- When GitHub status cannot be observed, use a subdued capability notice such as `PR status unavailable`, not a peer of the branch name. Preserve its actionable tooltip.

**Live state (right)**
- Keep the sole state dot and explicit state text here: `Working`, `Ready`, `Needs reply`, `Needs approval`, or error text.
- Keep context-window and elapsed/phase details adjacent to live state, not mixed into identity.

At 1440×900 the normal two-line form should fit without horizontal scrolling. Values may truncate, but every truncated semantic value needs an accessible full label and useful tooltip; labels themselves should not truncate before values.

### Sidebar

- Replace UUID project tabs with a canonical human project/repository label. If two projects have the same leaf name, disambiguate with the nearest meaningful parent, not an opaque worktree ID.
- Keep the conversation's human title primary.
- Keep one at-a-glance state dot and one consistent mode label.
- Use secondary context for project/working-directory disambiguation where needed; avoid repeating project context when the list is already filtered to one project unless needed for ambiguity.
- Retain recency and message count, but reduce their visual priority relative to title and actionable state.
- Continue to use cached PR data only for cheap list summaries; do not make each row perform live PR observation.

### Grounding panel

- Stop independently synthesizing and repeating `project · branch` from `rootPath` and `branchName`.
- Present the panel's own role and root context: e.g. `Grounding` / `Files in phoenix-ide`, or simply `Files` with the summarized root.
- Branch and PR identity belong in the canonical StateBar. The full server path may remain in a tooltip/copy affordance because it is operational detail.
- Keep collapsed badges focused on panel capabilities (`Files`, `MCP`, `Skills`, `Tasks`, `Work`) rather than conversation identity.

### New-conversation project selection

- Apply the same canonical human project-label derivation to project tabs/chips on `/new`.
- A selected project and the selected directory must read as one coherent choice. Project filtering must not make a home-directory value look like that project's root.

## Owning invariant

Every user-facing representation of conversation context must derive from one typed display model that distinguishes semantic identities:

- conversation title
- project/repository label
- mode
- task title
- base branch
- active branch
- worktree/path detail
- associated PR summary
- live activity state

Opaque IDs remain valid keys and deep-link handles, but must not be eligible fallback labels. Components may intentionally consume subsets of this model, but must not re-derive competing labels from raw paths.

## Interaction map

- Backend/SSE `Conversation` fields (`slug`, `project_name`, `cwd`, `worktree_path`, `task_title`, `base_branch`, `branch_name`, `conv_mode_label`, cached PR) → conversation store/hooks → shared conversation identity display model.
- Shared identity model → `ConversationList`, `/new` project tabs, `StateBar`, and grounding/file panel header.
- Focused conversation ID/mode/branch → `useConversationPrStatus` → active PR badge/selector in StateBar and actions in the work-actions rail.
- Conversation presentation/runtime state → existing StateBar phase derivation → sole live-state indicator. Do not merge this into navigation or grounding state.

## Proposed implementation scope

### Shared derivation

Introduce a small typed conversation identity view model/helper rather than another set of component-local conditions. Start from:

- `ui/src/components/ConversationList.tsx`: `compactContextLabel`, `displayTitleFromConversation`, project tabs/row metadata
- `ui/src/components/StateBar.tsx`: `getProjectName`, `statebar-line1`, `statebar-line2`, PR rendering
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx`: `projectName`, `fe-subtitle`
- the `/new` project selection producer/consumer discovered during implementation
- `ChainWorkIdentityBlock` only where sharing the model removes duplicate work identity rules without broadening chain-page layout work

The helper must reject UUIDs, long hashes, and generated worktree directory names as display labels structurally/centrally. Prefer backend `project_name` only when it is human-meaningful; otherwise derive the repository/root identity rather than the managed-worktree leaf.

### Rendering

Restructure desktop StateBar markup/CSS around identity, work context, and live state. Preserve existing model picker, PR selection authority, context indicator, cancellation/steering behavior, and live phase derivation.

Update sidebar/project-tab and grounding-header rendering to consume the shared model at the appropriate level of detail.

### Specs

Update the normative requirements before or with code:

- `specs/conversation-ui/requirements.md`, especially `REQ-CONV-001`, `REQ-CONV-007`, `REQ-CONV-012`, and `REQ-CONV-016`, to require human-meaningful identity and prohibit opaque IDs as display labels while preserving StateBar activity ownership.
- Reconcile presentation ownership with `specs/pr-association/` and `specs/work-actions-bar/`: StateBar shows active PR identity/status; work-actions retains freshness/actionability.
- Update affected executive status/coverage documents.
- No Allium spec is warranted for this UI-only information hierarchy unless implementation reveals a lifecycle/state transition change.

## Acceptance criteria

1. No UUID, long hash, or generated managed-worktree directory name is shown as a project, conversation, or repository label in desktop chrome when a semantic identity can be derived.
2. At 1440×900, Direct, Explore, Work, and Branch conversations each expose a coherent current-state summary without relying on hover to understand the basic meaning.
3. Work/Branch state uses explicit wording for active branch and base branch; the ambiguous `base ← branch` presentation is removed.
4. A focused associated PR is shown as a semantic PR identity/status link; multiple candidates use the existing explicit active selection model; unavailable observation is clearly a capability notice.
5. Live agent activity remains represented exactly once, in the StateBar, with explicit state text.
6. The grounding header no longer competes with the StateBar as an independent branch/project identity summary.
7. Sidebar rows and `/new` project selection use the same canonical project naming rules as the focused conversation.
8. Full technical paths remain available where useful via tooltip/details/copy, without being promoted as primary labels.
9. Existing desktop navigation, model switching, PR selection, work actions, grounding collapse, and conversation switching continue to work.
10. UI labels and mode casing are consistent across sidebar and StateBar.

## Validation

### Automated

- Add focused unit tests for the shared identity derivation, including:
  - explicit semantic `project_name`
  - UUID-valued `project_name`
  - managed worktree path with UUID leaf
  - normal repository cwd
  - task title + branch + base branch
  - Direct/Explore without branch
  - missing/partial identity fields
  - duplicate project leaf-name disambiguation if supported by the project-tab producer
- Update `ConversationList.test.tsx` for human project labels, row hierarchy, and cached PR summaries.
- Update `StateBar.test.tsx` for all modes, explicit branch language, PR states (none/open/multiple/unavailable), semantic status text, and accessible full labels.
- Add/extend focused tests for `FileExplorerPanel` header behavior and `/new` project tabs.
- Run `./dev.py codegen` only if wire types change; a UI-only display model should not require wire changes.
- Run `./dev.py check`.

### Browser journey (required; no Ladle)

At 1440×900 in the seeded live dev instance:

1. Open `/new`; verify project tabs and directory choice contain no opaque IDs and agree semantically.
2. Create a new Direct conversation; verify name, project, mode/model, and working/ready state.
3. Open seeded Explore, Work, and Branch conversations; verify the same hierarchy adapts by mode.
4. Exercise no PR, one open PR, multiple associated PRs, and PR-observation-unavailable states where seed/live data permits.
5. Collapse/expand grounding and sidebar; verify identity remains understandable and state dots remain available per `REQ-CONV-016`.
6. Open a dense conversation and switch among conversations; verify the chrome remains stable, readable, and unambiguous.
7. Capture before/after 1440×900 screenshots for Direct, Work, and Branch states.

## Risks

- `project_name` may already contain a managed-worktree leaf from the backend. Central filtering fixes display consistency, but the implementation must verify whether the producer can provide a better repository label rather than relying only on path heuristics.
- PR presentation must not collapse the distinction between cached list data, live focused status, active selection, and work-action freshness.
- StateBar is already a dense component with working-phase timing and model-picker behavior; isolate identity derivation and avoid changing timer/heartbeat logic.
- Narrower desktop widths may force progressive disclosure. Optimize the requested 1440×900 target first, then ensure existing responsive behavior does not regress.

## Explicit non-goals

- Mobile redesign.
- Conversation message content, chapter navigation, tool output, or composer redesign.
- Grounding panel feature/content redesign beyond removing duplicate identity and clarifying its header.
- Ladle fixture work.
- New PR discovery, GitHub mirroring, or changing active-PR authority.
- Backend worktree/branch lifecycle changes.
- Changing UUIDs used internally, in persistence, or as unavoidable deep-link handles.
- Moving live activity into the top conversation navigation strip.
