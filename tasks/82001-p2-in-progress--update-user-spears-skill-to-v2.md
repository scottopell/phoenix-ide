# Update user-level spEARS skill to v2

Replace the stale user-level spEARS skill at `~/.claude/skills/spears` with the v2 copy from the local checkout at `~/dev/spears/skills/spears`.

## Plan

1. Inspect the existing user-level skill and the source checkout.
2. Replace the contents of `~/.claude/skills/spears` with `~/dev/spears/skills/spears`.
3. Verify the installed skill contains the v2 files, including:
   - `SKILL.md`
   - `references/authoring.md`
   - `references/adr-guide.md`
   - `references/design-philosophy.md`
   - `references/discovery.md`
   - `references/ears-guide.md`
   - `references/traceability.md`
   - `references/validation.md`
   - `references/worked-examples.md`
   - `adrs/`
4. Confirm old v1-only reference files are no longer present in the installed copy.

## Notes

This is a user-level skill installation update, sourced from the local `~/dev/spears` checkout, not a Phoenix source-code change.
