# ADR-262: Retire the delivery roadmap while preserving historical evidence

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-ROADMAP-001 through REQ-ROADMAP-014

## Context

The user asks to “deprecate roadmap as part of this, it’s so stale and
infrequently used.” The generated Issue #806 can be freshly rendered while its
underlying outcome claims are weeks old. Maintaining a second coordination
projection has not made those claims dependable. Ember's proposed durable
tracking list addresses forgotten user intent, not the authority for requirements,
review state or shipped behavior.

The updater checks repository configuration and an activation marker, not whether
the Issue is open. An execution already past its guard can overwrite an archive
notice. Closing the Issue or merely removing agent instructions is insufficient.

## Options considered

1. **Keep ADR-069's typed records and refresh them more often.** This retains
   posting duties and the stale second projection the user wants to retire.
2. **Replace it with a new executive projection or bulk-import every row.** This
   risks reviving stale commitments and creating another status authority before
   the smaller tracking foundation has proved useful.
3. **Retire updating and posting, retain the final projection and comment log.**
   Current obligations are verified selectively against their owning authorities.

## Decision

Choose option 3 and supersede ADR-069. The entire retained roadmap protocol,
including all REQ-ROADMAP-001–014 SHALL statements and posting/activation rules,
is historical and non-normative. Preserve its original text and the Issue's final
projection/comments for investigation; stop agent reading/posting duties.

Disable the updater, quiesce executions, clear both roadmap repository variables,
remove the Issue activation marker, and add a historical-only notice. Remove the
source workflow, reducer and its dedicated tests/CI step. Confirm the live cutoff
separately from the branch diff. Do not delete the Issue or its comments.

No bulk import or dual-write period is introduced. Specifications/Allium, ADRs,
task filenames, PRs, exact deployed evidence and current user instructions retain
their respective authority. Tracking can retain links and user intent without
mirroring those authorities or changing an old pause merely by omitting a row.

## Consequences

- **Positive:** agents no longer spend effort maintaining a misleading second
  coordination projection; old evidence remains accessible.
- **Negative:** there is no generated project-wide milestone inventory. Current
  ownership and delivery must be checked through active tasks, PRs and coordination.
- **Neutral:** old links and posted records remain historical evidence. Retirement
  does not declare every old outcome complete, cancelled or newly commissioned.

## References

- [ADR-069](069_roadmap-records-are-typed-facts-projected-milestone-first.md)
- [Retained protocol](../roadmap/requirements.md)
- [Retirement status and evidence](../roadmap/executive.md)
- [Planning PR #1082](https://github.com/scottopell/phoenix-ide/pull/1082)
- [Historical Issue #806](https://github.com/scottopell/phoenix-ide/issues/806)
