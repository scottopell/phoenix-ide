# Skills UI Design

This document describes the technical architecture for the Skills
panel and Skill viewer, implementing `specs/skills-ui/requirements.md`.

## Component Boundary

```
┌─ Conversation page (specs/conversation-ui/) ────────────────────┐
│                                                                │
│   ┌─ SkillsPanel.tsx (this spec) ─────────────────────────┐   │
│   │  collapsed header → "Skills · 12 available"           │   │
│   │  expanded → groups by source                          │   │
│   │    Built-in        (pulled to top)                    │   │
│   │    <project name>  (per-repo .claude/skills)          │   │
│   │    User            (~/.claude/skills, ~/.agents/...)  │   │
│   │                                                       │   │
│   │  click row → onSkillClick(skill)                      │   │
│   └───────────────────────────────────────────────────────┘   │
│                                                                │
│   ┌─ SkillViewer.tsx (this spec) ─────────────────────────┐   │
│   │  /<name> header                                       │   │
│   │  description (from frontmatter)                       │   │
│   │  details: project / source / args / path              │   │
│   │  prompt (frontmatter stripped)                        │   │
│   │                                                       │   │
│   │  "Insert /<name> into input" → CustomEvent            │   │
│   │      consumed by InputArea                            │   │
│   └───────────────────────────────────────────────────────┘   │
│                                                                │
│   ┌─ InputArea.tsx (specs/conversation-ui/) ──────────────┐   │
│   │  listens for window 'phoenix:insert-draft' events     │   │
│   └───────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────┘
                             ↕ HTTP
┌─ Backend ──────────────────────────────────────────────────────┐
│  GET /api/conversations/:id/skills → SkillEntry[]             │
│  GET /api/files/read?path=...      → SKILL.md contents        │
└────────────────────────────────────────────────────────────────┘
```

## API Surface

- `api.listConversationSkills(conversationId)` — returns `{ skills: SkillEntry[] }` where each entry carries `{ name, description, source, path, argument_hint? }`. `source` is either the literal `"builtin"` (for skills bundled with the Phoenix binary and extracted at startup) or the discovery directory marker that the walk hit — typically `".claude/skills"` or `".agents/skills"`. It is NOT a kind enum like `"builtin"` / `"filesystem"`; the SkillViewer renders the raw value in the Source field so the user can tell exactly where a skill came from. The backend runs the discovery walk rooted at the conversation's `cwd` and returns the same catalog the LLM sees in its system prompt (per `specs/skills/`).
- `GET /api/files/read?path=<absolute>` — generic read endpoint used by `SkillViewer` for the SKILL.md body. Built-ins are extracted to disk at server startup (per `specs/builtin-skills/`), so they share the same read path as filesystem skills.

## State (`SkillsPanel`)

| State | Purpose | Driven by |
|---|---|---|
| `skills` (SkillEntry[]) | The catalog for the current conversation | `useEffect` on `[conversationId]` — fetches when conversation changes |
| `expanded` (boolean) | Whether the panel body is visible. Can be controlled (parent passes `expanded` + `onToggleExpanded`) or internal | Header click |
| `expandedGroups` (Set<string>) | Per-group collapse state | Group header click; defaults to all-open after first fetch |

Note: unlike the Tasks panel, the Skills panel fetches on conversation
change (not on expand). The trade-off: skills are typically a small
catalog (~10-30 entries) and the count is part of the collapsed
header summary, so we need the count up front. Tasks can grow to
hundreds of entries, justifying the defer-until-expanded pattern
there. Both choices are documented in the respective specs.

## State (`SkillViewer`)

| State | Purpose |
|---|---|
| `promptContent` (string?) | Frontmatter-stripped body |
| `loading` / `promptError` | Standard fetch lifecycle |

No raw-content split: unlike the task viewer's "Start working" path,
the skill viewer's "Insert into input" action only needs the skill
name, not its body.

## Group Labelling

`groupLabel(skill)` (`SkillsPanel.tsx:13-39`) computes the group name:

1. `source === 'builtin'` → `"Built-in"`
2. Filesystem skill: search the path for `.claude/skills` or `.agents/skills`, take the directory _above_ that marker as the group name
3. If the parent directory looks like `$HOME` (e.g. `/Users/<name>`, `/home/<name>`, or `~`) → `"User"`
4. Anything else → `"Other"` (defensive fallback for unexpected paths)

`groupSkills` then iterates the catalog, building a `Map<string, SkillEntry[]>`. Insertion order matters because `Map` preserves it: the first skill with a given group label sets the group's position. The function then explicitly reorders so `"Built-in"` is first regardless of where it appeared in the source array.

## "Insert into input" via CustomEvent

The viewer dispatches a global `CustomEvent`:

```ts
window.dispatchEvent(
  new CustomEvent('phoenix:insert-draft', {
    detail: { text: `/${skill.name} ` },
  }),
);
onBack();
```

The `InputArea` (owned by `specs/conversation-ui/`) listens for this
event at `InputArea.tsx:90-101` and **replaces** the entire draft via
`setDraft(text)`, then focuses the textarea. It does not insert at
the cursor or preserve any existing draft text. The decoupling
avoids prop-drilling a callback through SkillsPanel + SkillViewer;
both can sit anywhere in the tree without needing to know what
hosts them.

The event is fire-and-forget. There's no acknowledgement: if the
InputArea is unmounted (e.g. user is on a page that doesn't have one),
the dispatch is a no-op. That's the right trade — the viewer would
otherwise need to know about the InputArea's lifecycle, which
defeats the decoupling.

The trailing space in `/${skill.name} ` is deliberate: most
invocations need arguments, so the cursor lands where the user
would start typing.

## Why Not Use the InputArea Autocomplete?

`specs/inline-references/` already provides autocomplete when the user
types `/` in the InputArea. The Skills panel is the deliberate browse
alternative for users who don't remember the name they want. Both
paths converge on the same `/<name>` invocation, which `specs/skills/`
expands at message-send time.

The two surfaces serve different points in the journey:

- **Autocomplete** (`specs/inline-references/`): I know what skill I want, I just need name completion.
- **Panel + viewer** (this spec): I don't know what's available; I want to see the catalog and read prompts before committing.

## Cross-Spec Hand-Offs

- **Skill catalog** (`SkillsPanel.tsx:79-105`): the `listConversationSkills` API runs the same discovery walk that `specs/skills/` REQ-SK-* defines for LLM-side catalog rendering. The panel and the system prompt see the same set.
- **SKILL.md read** (`SkillViewer.tsx:50-80`): for built-ins, `specs/builtin-skills/` extracts assets to disk at server startup so they're readable via the same `/api/files/read` endpoint as filesystem skills.
- **Insert into input** (`SkillViewer.tsx:82-89`): the `phoenix:insert-draft` CustomEvent is consumed by the InputArea, owned by `specs/conversation-ui/`. The decoupling is intentional.

## Open Questions

None. The UI is read-only on the backend (the only mutation is the
draft insertion, which is local UI state). The two tradeoffs in this
spec — fetch-on-mount vs defer-until-expanded, and CustomEvent vs
callback prop — are documented above with their rationale.
