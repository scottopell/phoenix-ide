# Skills UI — Executive Summary

## Scope and Boundary

This spec governs the user-facing **skills panel and skill viewer** inside a conversation: the collapsible Skills panel that lists what slash-commands the agent has available here, and the single-skill detail view that shows me the prompt body and lets me insert the invocation into my message.

The skills system itself — discovery, invocation, frontmatter parsing, the `/skill-name` syntax, the catalog rendering rules — is owned by `specs/skills/` and `specs/builtin-skills/`. This UI spec consumes that catalog but does not define it.

**In scope here (user-facing experiences):**
- Seeing what skills are available in the current conversation's working directory
- Telling at a glance which skills are built-in vs project-local vs user-level
- Reading a skill's description, prompt body, and arguments before using it
- Inserting a skill invocation (e.g. `/distill `) into my current message draft
- Defer-load: don't pay the cost on every conversation mount

**Owned by other specs:**
- `specs/skills/` — backend skill discovery, frontmatter parsing, slash-command invocation expansion, catalog rendering for the LLM system prompt
- `specs/builtin-skills/` — embedded skill assets, startup extraction to disk, override precedence (filesystem skills shadow built-ins of the same name)
- `specs/conversation-ui/` — the parent layout that hosts the Skills panel; the `InputArea` that consumes the inserted invocation
- `specs/inline-references/` — `@file` / `/skill` autocomplete inside the input (the "type / and see suggestions" path; the Skills panel is the "browse the catalog" path)

## Why It Exists

Slash-command skills are powerful but non-obvious — without a UI surface, users have to know a skill exists before they can use it. The Skills panel makes the catalog browsable inline; the SkillViewer turns "what does `/distill` actually do?" into a one-click answer. The "Insert into input" affordance closes the loop: discover → read → use, without ever leaving the conversation.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-SKILLS-UI-001:** See What Skills Are Available Here | ✅ Complete | `ui/src/components/SkillsPanel.tsx:79-105` (fetch on conversation change), `:131-141` (collapsible header with count) |
| **REQ-SKILLS-UI-002:** Tell Built-in vs Project vs User at a Glance | ✅ Complete | `:13-39` (groupLabel: built-in pulled to top, then per-project, then User), `:42-68` (groupSkills + insertion-order Map for stable rendering) |
| **REQ-SKILLS-UI-003:** Read a Skill's Prompt Before Using It | ✅ Complete | `SkillViewer.tsx:50-80` (fetch SKILL.md), `:91-145` (description + details + prompt body with frontmatter stripped) |
| **REQ-SKILLS-UI-004:** Insert a Skill Invocation Into My Message | ✅ Complete | `SkillViewer.tsx:82-89` (dispatch `phoenix:insert-draft` event), `:147-151` (insert button); InputArea consumes the event |
| **REQ-SKILLS-UI-005:** Hide When There's Nothing To Show | ✅ Complete | `:109-111` (panel returns null when no skills + collapsed) |

**Progress:** 5 of 5 complete. Read-mostly UI; one mutation path (insert into draft) goes through a CustomEvent rather than a callback prop, decoupling the panel from the input.

## Behavioural Specification

No Allium spec — the panel and viewer are read-only views with local UI state (group-expanded, loading) and no meaningful state machine. The "insert into input" action is a fire-and-forget event dispatch.

## Cross-Spec Cross-References

- `specs/skills/`: REQ-SK-* govern how skills are discovered (filesystem walk, frontmatter parse), how `/name` invocations expand at message-send time, how the catalog is rendered into the LLM system prompt. The Skills panel reads the same catalog the LLM sees.
- `specs/builtin-skills/`: built-in skills are embedded in the binary and extracted to disk at server startup, then discovered alongside filesystem skills. The panel renders them in a dedicated "Built-in" group pulled to the top of the list.
- `specs/inline-references/`: when the user types `/` in the InputArea, a different code path (autocomplete) suggests skills. The Skills panel is the deliberate "browse" alternative for users who don't remember the name. Both paths converge on the same `/name` invocation expansion.
- `specs/conversation-ui/`: the panel sits inside the conversation page's right rail / settings area. The layout is owned there; the panel's content is owned here.
