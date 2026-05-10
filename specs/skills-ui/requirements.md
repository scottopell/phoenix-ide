# Skills UI

## Scope

This spec governs the **Skills panel** and the **Skill viewer** inside a conversation: the surface that lists what slash-commands the agent has available, and the detail view that shows me the prompt body and lets me drop the invocation into my message. The skill discovery + invocation backend is owned by `specs/skills/` and `specs/builtin-skills/`. See `specs/skills-ui/executive.md` for the full boundary.

## User Story

As a Phoenix user inside a conversation, I want to discover what slash-command skills are available without leaving the page, see what each one actually does before I run it, and drop a skill invocation into my current message in one click — instead of having to remember every `/name` and what it expects.

## Transparency Contract

The Skills UI must let me confidently answer:

1. What skills can I use here?
2. Which are built-in vs project-specific vs from my user-level config?
3. What does `/<skill-name>` actually do (prompt + description)?
4. Does it take arguments?
5. Where does it come from (which file)?
6. How do I use it in my current message?

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-SKILLS-UI-001: See What Skills Are Available Here

WHEN I'm inside a conversation
THE SYSTEM SHALL show a collapsed "Skills" panel header in the conversation's chrome
AND SHALL show the count in that header (e.g. "Skills · 12 available")

WHEN I expand the panel
THE SYSTEM SHALL render the catalog of skills the agent has access to in this conversation, the same set the LLM sees in its system prompt

WHEN there are zero skills AND the panel is collapsed
THE SYSTEM SHALL render nothing — no header for an empty catalog

**Rationale:** Skills are reference data, not something the user always interacts with. Headline-with-count is the right disclosure (silent until I look). The "hide entirely when empty + collapsed" rule keeps the conversation chrome clean for repos that don't use skills at all; the header reappears the moment any skill is discovered.

---

### REQ-SKILLS-UI-002: Tell Built-in vs Project vs User at a Glance

WHEN the catalog has built-in skills
THE SYSTEM SHALL render them in a "Built-in" group pulled to the top of the list

WHEN the catalog has filesystem skills from project directories (e.g. `<repo>/.claude/skills/`, `<repo>/.agents/skills/`)
THE SYSTEM SHALL group them under their project name (the directory above `.claude/skills` / `.agents/skills`)

WHEN the catalog has skills under `$HOME/.claude/skills/` or `$HOME/.agents/skills/`
THE SYSTEM SHALL group them under "User"

WHEN displaying a group
THE SYSTEM SHALL show the group name, count, and let me collapse/expand it independently

**Rationale:** Source matters: a built-in is shipped by Phoenix and known-stable, a project skill belongs to the team I'm working with, a User skill is something I added myself for personal workflow. Grouping mirrors that mental hierarchy. Built-in pulled to the top because those are the longest-lived, most-likely-to-be-the-thing-I-want skills; per-project below because they're contextual to where I am; User last because it's the catch-all.

---

### REQ-SKILLS-UI-003: Read a Skill's Prompt Before Using It

WHEN I click a skill in the panel
THE SYSTEM SHALL open a detail view showing:
- The slash invocation (`/<skill-name>` as the header)
- A short description (from frontmatter)
- A "Project" field naming the origin (Built-in / project name / User)
- A "Source" field showing the discovery root — either the literal `builtin` (for skills bundled with the Phoenix binary) or the directory marker the discovery walk hit (e.g. `.claude/skills`, `.agents/skills`)
- An "Args" field, when the skill declares an `argument_hint`
- The "Path" — for filesystem skills, the directory containing `SKILL.md`
- The full prompt body, with YAML frontmatter stripped

WHEN the prompt body fails to load
THE SYSTEM SHALL show the error inline in the Prompt section without losing the rest of the metadata

WHEN I want to go back
THE SYSTEM SHALL provide an obvious back button that returns me to the panel list view

**Rationale:** Slash commands are user-invokable LLM prompts; running one without reading it is exactly the "I don't know what's about to happen" experience the transparency contract is meant to prevent. The viewer makes the prompt the centrepiece and the metadata supporting context. The Source field reflects what `SkillEntry.source` actually carries from the backend (a discovery directory marker or `"builtin"`) rather than a kind enum — this matches the data so the rendered field doesn't lie. Showing the path for filesystem skills is the small detail that lets me find the file when I want to edit it.

---

### REQ-SKILLS-UI-004: Drop a Skill Invocation Into My Draft

WHEN I'm viewing a skill
THE SYSTEM SHALL provide an "Insert /<skill-name> into input" button at the foot of the viewer

WHEN I click it
THE SYSTEM SHALL set the conversation's message draft to the text `/<skill-name> ` (with trailing space)
AND SHALL return me to the panel list view (or close the viewer)

WHEN the draft already contained text
THE SYSTEM SHALL replace it (this is what the InputArea consumer of `phoenix:insert-draft` does today). The user explicitly chose to use this skill from the browse path; preserving prior draft text would conflate two intents

**Rationale:** Discoverability is half the value; the other half is "use what I just discovered." Without the button, I'd have to read the name, dismiss the viewer, and type it manually — every step a chance to typo or forget. The trailing space makes the cursor land where I'd start typing arguments, which is where most invocations need to continue. The draft-replace contract is deliberate: the browse path is "I went to find this skill on purpose"; if I had a different message in flight I'd dismiss the viewer rather than confirm. (Future iteration could prompt-confirm before overwriting non-empty drafts; the present spec accepts the simple replace.)

---

### REQ-SKILLS-UI-005: Don't Mislead Me When the Catalog Changes

WHEN I navigate to a different conversation
THE SYSTEM SHALL re-fetch the skill list for the new conversation

WHEN the new conversation has no skills (response is empty)
THE SYSTEM SHALL hide the panel header (REQ-SKILLS-UI-001) so the "12 available" count from the previous conversation does not stay visible after the new fetch resolves

WHEN the new fetch is in flight
THE SYSTEM MAY transiently show the previous conversation's count until the response arrives — the spec does not require a synchronous clear-on-conversationId-change. The transient stale during a single network round-trip is preferable to a flicker-to-empty followed by re-population

**Rationale:** Skills are per-conversation (the discovery walk roots from the conversation's working directory). A stale catalog from a different repo would lie about what's invocable; re-fetching per conversation is the correct trade. The "may transiently stale during fetch" clause documents the actual implementation behaviour: `SkillsPanel.tsx:79-105` keeps the previous `skills` state until the new response replaces it. A future tightening could clear immediately on `conversationId` change to avoid the brief stale window; the present spec accepts the impl's trade for fewer flickers.
