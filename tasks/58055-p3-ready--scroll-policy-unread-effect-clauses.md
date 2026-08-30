scroll_policy.allium states an unread effect unconditionally alongside every unread state postcondition: 14 rules carry `ensures: ScrollEffect.created(..., kind: clear_unread|show_unread)` directly beneath `ensures: policy.unread = false|true`.

The reducer emits the effect only on a transition — `unreadEffects` returns nothing when the value is unchanged — so a rule reached with unread already at its target value satisfies the state postcondition but emits no effect. `DownwardArrivalAtBottomConfirmsTailReturn` is reachable that way routinely: a reader scrolls up, no new content arrives, they scroll back down.

Two possible resolutions, and picking between them needs a decision about what `ScrollEffect.created` means in this file rather than a local edit:

1. The effect clauses are redundant with the adjacent state postconditions and should be deleted, with the state-to-effect edge stated once. This makes the spec smaller and removes 14 independent over-constraints.
2. The convention is declarative ("this rule is what creates such effects") rather than per-application, in which case nothing is wrong and the convention deserves a note in the file so the next reader does not read it as a contradiction.

12 of the 14 sites predate PR #700; the pattern was copied into the two rules that branch added. Raised by automated review on #700 and deliberately not resolved there, since a sweeping change to a normative artifact should not ride on a scrolling fix.
