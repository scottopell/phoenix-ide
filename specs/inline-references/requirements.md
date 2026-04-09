# Inline References

## User Story

As a user, I need to embed file and skill references directly in my message so that I can give the AI precise context — either by explicitly loading content myself (`@file`, `/skill`) or by pointing the AI at a path and letting it decide how much to read (`./path`).

## Requirements

### REQ-IR-001: Include File Contents by Reference

WHEN a user includes `@path/to/file` anywhere in their message
THE SYSTEM SHALL include the referenced file's contents in the context delivered to the AI

WHEN the referenced file does not exist or cannot be read
THE SYSTEM SHALL notify the user before sending and SHALL NOT deliver a broken reference to the AI

WHEN a message contains multiple `@` references
THE SYSTEM SHALL resolve all of them before delivery

**Rationale:** Users frequently need the AI to reason about specific files. Typing `@src/main.rs` is faster and less error-prone than copying file contents manually or asking the AI to find the file itself.

---

### REQ-IR-002: Load Skill Context by Name

WHEN a user sends a message beginning with `/skill-name`
THE SYSTEM SHALL load the named skill's full instructions into the context delivered to the AI

WHEN the named skill does not exist
THE SYSTEM SHALL notify the user before sending and SHALL NOT deliver a broken reference to the AI

**Rationale:** Skills define reusable instruction sets (writing style, review process, deployment steps). Typing `/writing-style` is the explicit, direct way to load that context — rather than relying on the AI to notice the skill is relevant.

---

### REQ-IR-003: Pass Additional Context to Skill Invocations

WHEN a user sends `/skill-name` followed by additional text
THE SYSTEM SHALL make that additional text available within the skill's instructions as substitutable content

WHEN a skill's instructions contain no substitution placeholder and additional text is provided
THE SYSTEM SHALL append the additional text to the skill instructions so the AI still receives it

WHEN no additional text follows `/skill-name`
THE SYSTEM SHALL load the skill's instructions unmodified with no substitution performed

**Rationale:** Skills are context loading by default. Additional text is opt-in enrichment — useful for directing a skill (e.g. `/review src/auth.rs`) without making it a required argument.

---

### REQ-IR-004: Discover Files via Inline Autocomplete

WHEN a user types `@` in the message input
THE SYSTEM SHALL show an inline autocomplete dropdown of files in the conversation's working directory

WHEN a user types `./` anywhere in the message input
THE SYSTEM SHALL show the same inline autocomplete dropdown of files in the conversation's working directory

WHEN the user continues typing after either trigger
THE SYSTEM SHALL filter suggestions by fuzzy match on file path

WHEN the user selects a suggestion from the `@` trigger
THE SYSTEM SHALL insert the completed `@path/to/file` reference at the cursor and dismiss the dropdown

WHEN the user selects a suggestion from the `./` trigger
THE SYSTEM SHALL insert the completed `./path/to/file` path at the cursor and dismiss the dropdown

WHEN the user presses Escape
THE SYSTEM SHALL dismiss the dropdown without inserting

**Rationale:** Users don't memorize exact paths. Both `@` and `./` trigger the same file picker, but carry different intent: `@` promises to expand the file's contents at send time, while `./` simply completes the path and leaves it as literal text. Sharing the autocomplete surface keeps the interaction consistent while preserving the semantic distinction.

---

### REQ-IR-005: Discover Skills via Inline Autocomplete

WHEN a user types `/` at the beginning of the message input
THE SYSTEM SHALL show an inline autocomplete dropdown listing available skills

WHEN the user continues typing after `/`
THE SYSTEM SHALL filter suggestions by fuzzy match on skill name

WHEN a skill has an argument hint defined
THE SYSTEM SHALL display it alongside the skill name and description in the dropdown
AND after the user selects the skill, THE SYSTEM SHALL show the hint as ghost text in the input field

WHEN the user selects a suggestion
THE SYSTEM SHALL insert `/skill-name ` at the cursor and dismiss the dropdown

WHEN the user presses Escape
THE SYSTEM SHALL dismiss the dropdown without inserting

**Rationale:** Users may not recall the exact name of a skill. Autocomplete surfaces skill names with their descriptions so the user can recognize the right one. The argument hint communicates that additional context is welcome, without implying it is required.

---

### REQ-IR-006: Preserve Original Shorthand in Conversation History

WHEN a message containing expansion references is sent
THE SYSTEM SHALL store and display the original shorthand the user typed (e.g. `@src/main.rs`, `/writing-style`)
AND SHALL deliver the fully expanded content to the AI

**Rationale:** Conversation history should reflect what the user actually did, not a wall of injected file contents or skill instructions. The shorthand is human-readable; the expanded form is for the AI.

---

### REQ-IR-007: Graceful Handling of Unresolvable Expansion References

WHEN a message contains `@` followed by a token that looks like a file path
(contains `/` or `.` with a recognized file extension)
AND the referenced file does not exist
THE SYSTEM SHALL present the user with a clear error identifying the missing file
AND SHALL NOT send the message until the reference is removed or corrected

WHEN a message contains `@` followed by a token that does NOT look like a
file path (bare word, email address, `@mention`-style text)
THE SYSTEM SHALL treat the `@` as literal text
AND SHALL send the message without expansion or error

WHEN a message begins with `/skill-name` that matches no available skill
THE SYSTEM SHALL treat the `/` as literal text (no error, no block)

**Rationale:** `@` appears in email addresses, `@mention` conventions, code
annotations (`@param`, `@override`), and casual text. Blocking send for
these false positives is more disruptive than the original risk of silently
broken references. The heuristic (path-like = intentional reference) catches
real file references while letting casual `@` usage pass through. `/skill`
already falls through silently when no skill matches (the token is ignored).
`./path` references are not validated at send time (see REQ-IR-008).

---

### REQ-IR-008: Reference Files by Path Without Expansion

WHEN a user types `./` anywhere in a message and completes a path using autocomplete
THE SYSTEM SHALL send the message with the literal path text intact
AND SHALL NOT read or inject the file's contents

WHEN the path typed after `./` does not match any file in the autocomplete results
THE SYSTEM SHALL still allow the message to be sent without error

**Rationale:** Sometimes the user wants to point the AI at a file and let it decide how and how much to read — a full read may be wasteful for a large file, or the agent may only need a specific function. By sending `./src/auth.rs` as literal text, the user delegates the read strategy to the agent. Because no server-side expansion occurs, there is nothing to validate or block on; the agent handles any path resolution itself.
