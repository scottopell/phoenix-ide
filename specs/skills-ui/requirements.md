# Skills UI

## Scope

This spec governs the **Skills panel** and the **Skill viewer** inside a conversation: the surface that lists what slash-commands the agent has available, and the detail view that shows me the prompt body and lets me drop the invocation into my message. The skill discovery + invocation backend is owned by `specs/skills/` and `specs/builtin-skills/`. See `specs/skills-ui/executive.md` for the full boundary.

## User Story

As a Phoenix user inside a conversation, I want to discover what slash-command skills are available without leaving the page, see what each one actually does before I run it, and drop a skill invocation into my current message in one click — instead of having to remember every `/name` and what it expects.

## Transparency Contract

The Skills UI must let me confidently answer:

1. What skills can I use here?
2. Which are built-in vs workspace-specific vs from my user-level config?
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

### REQ-SKILLS-UI-002: Tell Built-in vs Workspace vs User at a Glance

WHEN the catalog has built-in skills
THE SYSTEM SHALL render them in a "Built-in" group pulled to the top of the list

WHEN the catalog has filesystem skills from workspace directories (e.g. `<repo>/.claude/skills/`, `<repo>/.agents/skills/`)
THE SYSTEM SHALL group them under their workspace name (the directory above `.claude/skills` / `.agents/skills`)

WHEN the catalog has skills under `$HOME/.claude/skills/` or `$HOME/.agents/skills/`
THE SYSTEM SHALL group them under "User"

WHEN displaying a group
THE SYSTEM SHALL show the group name, count, and let me collapse/expand it independently

**Rationale:** Source matters: a built-in is shipped by Phoenix and known-stable, a workspace skill belongs to the working context I'm in, and a User skill is something I added myself for personal workflow. Grouping mirrors that mental hierarchy. Built-ins appear first because they are longest-lived; workspace skills follow because they are contextual; User remains the catch-all.

---

### REQ-SKILLS-UI-003: Read a Skill's Prompt Before Using It

WHEN I click a skill in the panel
THE SYSTEM SHALL open a detail view showing:
- The slash invocation (`/<skill-name>` as the header)
- A short description (from frontmatter)
- An "Origin" field naming the source group (Built-in / workspace name / User)
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

WHEN the draft already contains text
THE SYSTEM SHALL replace it because choosing a skill from the browse path is one explicit draft intent

**Rationale:** Discoverability is half the value; the other half is using what I just discovered. The trailing space places the cursor where arguments begin. Replacing the draft keeps browse insertion as one unambiguous intent rather than merging unrelated text.

---

### REQ-SKILLS-UI-005: Don't Mislead Me When the Catalog Changes

WHEN I navigate to a different conversation
THE SYSTEM SHALL re-fetch the skill list for the new conversation

WHEN the new conversation has no skills (response is empty)
THE SYSTEM SHALL hide the panel header (REQ-SKILLS-UI-001) so the "12 available" count from the previous conversation does not stay visible after the new fetch resolves

WHEN the new fetch is in flight
THE SYSTEM MAY transiently show the previous conversation's count until the response arrives — the spec does not require a synchronous clear-on-conversationId-change. The transient stale during a single network round-trip is preferable to a flicker-to-empty followed by re-population

**Rationale:** Skills are per-conversation because discovery roots from the conversation's working directory. Re-fetching prevents a settled catalog from another workspace from claiming skills are invocable here; permitting bounded in-flight staleness avoids an unnecessary empty-state flicker.
