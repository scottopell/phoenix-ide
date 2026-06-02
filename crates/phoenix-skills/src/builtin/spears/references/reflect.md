# Context Window Reflection

Use this workflow at the end of a work session to produce a continuation prompt
that captures progress and enables seamless pickup in a fresh context.

## When to Use

- Context window is getting long and work isn’t finished
- User asks for a summary or handoff prompt
- Ending a session with outstanding work

## Process

Do NOT make tool calls.
Use extended thinking to reflect on the conversation, then produce a
continuation prompt.

## What to Capture

1. **Progress Made**: Where are we relative to any phased plan or roadmap?
   If no explicit phases exist, summarize progress against requirements.

2. **Current State**: What was actively being worked on when the session is
   ending?

3. **Next Steps**: What should be picked up next?
   Be specific about which requirements or tasks remain.

4. **Worth Following Up On**: Anything noticed during this session that deserves
   attention:
   - Failing tests encountered
   - Dead code or technical debt spotted
   - Inconsistencies in the codebase
   - Unresolved questions or decisions
   - Potential issues observed but not the focus

## Guidelines

- Trust `specs/**/executive.md` as the temporal link -- it reflects current
  reality
- Trust `specs/**/*.md` as the authoritative source over all other documentation
- Reference spEARS requirement IDs (REQ-*) as the primary unit of work
- Keep the continuation prompt minimal on context -- important info is already
  in the spec documents
- Focus on key insights specific to this session, not general project background
- The prompt should clearly lay out what’s next and help the next context pick
  up seamlessly

## Output Format

After reflection, output the continuation prompt in a fenced code block:

```text
<your continuation prompt here>
```

The continuation prompt should be immediately usable to resume work in a fresh
Claude Code context.
The user will copy it manually.
