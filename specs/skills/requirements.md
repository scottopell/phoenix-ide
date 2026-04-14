# Skill System

## User Story

As a user, I need reusable instruction sets (skills) that I can invoke by name
so that the AI follows specific workflows, coding standards, or review
processes without me re-explaining them every conversation.

As an LLM agent, I need to invoke skills programmatically when I recognize that
a discovered skill matches the task at hand, so that I can leverage specialized
instructions without the user manually typing a `/skill` prefix.

## Requirements

### REQ-SK-001: Frontmatter Separation

WHEN a skill is invoked (by user or by the LLM)
THE SYSTEM SHALL parse the SKILL.md file's YAML frontmatter and extract
metadata fields (name, description, argument hint, allowed tools, etc.)
AND strip the frontmatter block before delivering the skill body to the LLM

THE SYSTEM SHALL NOT include raw YAML frontmatter (`---` delimited blocks) in
the content delivered to the LLM

**Rationale:** Frontmatter is machine metadata for discovery and autocomplete,
not instructions for the AI. Including it wastes context tokens and confuses
the model with key-value pairs it can't act on.

---

### REQ-SK-002: Delivery Format Matches Invocation Semantics

WHEN a user invokes a skill via `/skill-name`
THE SYSTEM SHALL deliver the skill content as a user-role message marked as
system-generated (not typed by the user)

WHEN the LLM invokes a skill via the Skill tool
THE SYSTEM SHALL deliver the skill content as a tool result

THE SYSTEM SHALL NOT deliver skill content as a system prompt injection or
unstructured text dump

**Rationale:** The delivery format reflects who is issuing the instruction.
A user typing `/skill-name` is giving a directive — user-role message is the
right representation because the skill body replaces the user's message and
carries the same authority. The LLM calling the Skill tool is autonomously
fetching instructions mid-task — tool result is the right representation
because the model is asking "what should I do?" and receiving an answer, not
being commanded by the user. The distinction is semantically meaningful and
maps cleanly to how LLMs weight different message roles.

---

### REQ-SK-003: Base Directory Context

WHEN a skill is invoked
THE SYSTEM SHALL prepend "Base directory for this skill: {absolute path to
the skill's directory}" to the delivered content

**Rationale:** Skills often reference companion files (templates, checklists,
examples) stored alongside SKILL.md in a `references/` subdirectory. The base
directory line tells the AI where to find these files using its read tools,
enabling skills that are more than a single prompt -- they can point at a whole
directory of resources.

---

### REQ-SK-004: Argument Substitution

WHEN a skill is invoked with additional text (arguments)
AND the skill body contains `$ARGUMENTS`
THE SYSTEM SHALL replace `$ARGUMENTS` with the full argument string

WHEN the skill body contains positional placeholders (`$ARGUMENTS[1]`,
`$ARGUMENTS[2]`, or shorthand `$1`, `$2`)
THE SYSTEM SHALL replace each with the corresponding whitespace-delimited
argument token

WHEN the skill body contains named placeholders matching argument names
defined in the skill's frontmatter
THE SYSTEM SHALL replace each named placeholder with the corresponding
positional argument

WHEN a skill body contains no `$ARGUMENTS` placeholder and arguments are
provided
THE SYSTEM SHALL append the arguments to the skill body so the AI still
receives them

WHEN no arguments are provided
THE SYSTEM SHALL deliver the skill body unmodified

**Rationale:** Argument substitution lets skills be parameterized -- a
`/deploy staging` invocation can produce different instructions than
`/deploy production` without the user writing two separate skills.

---

### REQ-SK-005: Shared Expansion Logic

WHEN a skill is invoked by any path
THE SYSTEM SHALL use the same underlying expansion function: frontmatter
stripped, base directory prepended, arguments substituted

THE SYSTEM SHALL NOT have separate expansion logic per invocation path

**Rationale:** The content produced by skill expansion must be identical
regardless of trigger. Separate expansion logic would diverge. The delivery
wrapper (user-role message vs. tool result) differs by design per REQ-SK-002,
but the expanded body must not.

---

### REQ-SK-006: Skill Discovery

THE SYSTEM SHALL discover skills by scanning `.claude/skills/` and
`.agents/skills/` directories at each level from the conversation's working
directory up to the filesystem root

THE SYSTEM SHALL also scan immediate child directories of the working directory
for skills (handling the "projects directory" case)

THE SYSTEM SHALL also scan `$HOME/.claude/skills/` and `$HOME/.agents/skills/`
when `$HOME` is not an ancestor of the working directory

WHEN the same skill name appears at multiple levels
THE SYSTEM SHALL use the one closest to the working directory (more specific
overrides parent)

WHEN two paths resolve to the same file (via symlinks)
THE SYSTEM SHALL count them as one skill (first discovered wins)

WHEN two different files have identical content
THE SYSTEM SHALL count them as one skill (content-hash dedup)

**Rationale:** Skills are contextual -- a project-level `/build` skill
overrides a user-level `/build` because it's more specific. Symlink and content
dedup prevent the same skill from appearing twice when accessed through
different paths (common with symlinked skill directories).

---

### REQ-SK-007: Skill Metadata in System Prompt

THE SYSTEM SHALL include a catalog of discovered skill names, descriptions,
and argument hints in the system prompt so the LLM knows which skills are
available

THE SYSTEM SHALL NOT include skill bodies in the system prompt (they are loaded
on invocation, not preloaded)

**Rationale:** The LLM needs to know skills exist to suggest or invoke them,
but preloading all skill bodies into the system prompt would waste context
tokens. The catalog is a lightweight index; the full content is loaded on
demand.
