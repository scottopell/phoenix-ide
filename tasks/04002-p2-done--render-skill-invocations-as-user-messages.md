# Render slash-skill invocations as normal user messages

## Goal

When a user invokes a skill with a slash command (for example `/dogfood http://localhost:8042`), the transcript should look and feel like a normal user message. The skill token and arguments may receive subtle inline styling, but the message should not render as a separate bubble/capsule that reframes the user’s input as `skill: /name`.

## Current behavior

`ui/src/components/MessageList.tsx` has a special `case 'skill'` renderer. It renders the historical unit as a `.message.user`, but replaces the content with a `.skill-indicator` pill:

- label: `skill: /<name>`
- args in a separate muted span
- pill/capsule styling from `.skill-indicator`, `.skill-label`, and `.skill-trigger` in `ui/src/index.css`

This makes skill invocation visually distinct from ordinary user messages in a way that feels too heavy.

## Proposed behavior

1. Keep the outer message chrome identical to a normal user message (`You`, timestamp, normal user bubble styling).
2. Render the original trigger text inline, e.g. `/dogfood http://localhost:8042`, instead of the synthetic `skill: /dogfood` label.
3. Apply subtle inline styling only:
   - slash skill token (`/dogfood`) gets a small accent color/weight
   - arguments remain normal or slightly muted text
   - no pill background, no separate capsule, no `skill:` prefix
4. Preserve attachment/file chips for skill invocations.
5. Preserve useful accessibility/title text if helpful, but avoid visible special-case chrome.

## Implementation sketch

- In `ui/src/components/MessageList.tsx`:
  - Replace the `case 'skill'` content markup with normal message text markup.
  - Use `c.trigger` as the source of truth for displayed text.
  - If `trigger` is unavailable, fall back to `/${c.name || 'skill'}` plus extracted args where possible.
  - Split the rendered trigger into token and args using the existing `extractSkillArgs` helper or a small helper that returns `{ token, args }`.
- In `ui/src/index.css`:
  - Remove or stop using the pill-like `.skill-indicator` styling.
  - Add subtle inline classes such as `.skill-inline-token` and `.skill-inline-args` scoped under `.message-content`.
- Tests:
  - Update/add `MessageList` coverage so a `kind: 'skill'` unit renders the literal slash command text.
  - Assert it does not render `skill:` or `.skill-indicator`.
  - Assert attached files still render.

## Acceptance criteria

- A skill invocation in the transcript visually resembles a normal user message.
- The visible message content is the user’s slash command, not `skill: /name`.
- Skill token styling is subtle and inline.
- Arguments remain visible as part of the message text.
- Existing skill invocation behavior, attachments, and history rendering continue to work.
