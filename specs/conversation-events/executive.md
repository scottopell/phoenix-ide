# Conversation events — implementation and qualification

ADR-185 refines the locked installation boundary: the existing owner-propagated
GoalControlInstallation policy defaults Disabled; schema 135 does not install
a consumer. New local events distinguish disabled capture from legacy absence,
observed NoGoal and saved goal. Disabled adds no goal SQL, capture child row or
standalone event; Installed captures only newly inserted eligible subscription
events. Mandatory Close/federation remain unchanged. These gate amendments are
source-only; implementation/validation evidence is recorded separately below.

Implemented on the compact-conversation-events branch; not a deployment or merge claim. Durable metadata joins and typed presentation are separate from persisted diagnostic evidence. Available EventPresentation fields are flattened beside `status: available`; unavailable carries event_id/reason. EnrichedMessage.event_presentation is explicitly nullable. Genuine ts-rs exports are checked in, not handwritten.

## Bounded local goal facts — unexecuted source candidate

REQ-CEVT-006 and ADR-184 specify saved goal metadata on existing local watch
packets and standalone settled Create/State events. Rust wire metadata, local
GoalChanged outcome, runtime decoder and existing card details/search source are
authored but unexecuted. Mandatory Close and federation terminal-fact payloads
are unchanged. No generated export is hand-edited: authentic hosted codegen must
supply new WatchGoalCapture/State/Usage and changed event exports before corrected
UI qualification. The existing freshness guard is not waived.

Saved current facts, observed absence and legacy not-captured must remain
distinct; usage is explicitly unavailable. Execution outcomes keep their own
meaning, including “Turn finished.” Existing authenticated read paths,
unavailable fallback, shared redaction and source navigation remain authoritative.
New DB/API/wire/UI tests and migration preservation cases are authored, not run;
remaining qualification is tracked in `../conversation-goals/executive.md`.
Existing presentation evidence below does not qualify this extension. Only
source inspection, whitespace checks and separately authorized owned Rust
formatting occurred locally; no compiler, test, SQL, browser, activation or
Cargo metadata run supplies evidence for this candidate.

## Verification

| Contract | Evidence | Remaining boundary |
|---|---|---|
| REQ-CEVT-001 | Four DB tests; API batch completeness including 400/401 identities, fallback and outcome tests | Full workspace checks remain owed |
| REQ-CEVT-002/003 | 223 earlier focused UI tests; final c6b app TypeScript and 36 focused tests; actual dark/light browser rendering | Not physical-phone qualification |
| REQ-CEVT-004 | Ordinary library cargo check; eight event-presentation tests including five genuine export tests; SSE Complete/Tail/Preserve pending-event and redaction parity regression; genuine EnrichedMessage export | Full workspace codegen was interrupted by resource guard and remains owed separately; fixture browser is not actual backend acceptance |
| REQ-CEVT-005 | Eight rendered cases with 88 passing assertions, including distinct Global-to-ordinary source and return; only human prompt in chapter strip; no duplicate events | Real-server pagination/live/share browser campaign not performed |

Final rendered source: `c6b2292ab0e7e436202d6b0a5b365aff8dc512dc`. Matrix: ordinary/Global, dark/light, widths 1000/390. Recorded title foreground/background contrast: dark 6.848:1, light 4.877:1, with every ancestor opacity verified as 1. The scoped meta-wrapper override prevents generic opacity 0.85 from reducing title contrast. Native details, exact provenance/raw copying, hidden-provenance find, failure/missing fallback and local time passed. All eight cases recorded zero page errors, console errors/warnings, and escaped fixture requests. Existing Global watch/live-command fixture handlers were exercised without status errors; unknown/mutating requests still reject.

Independent whole-contract source review at 520a approved with no blocking counterexample. Follow-up read-only review at 82f approved routing/fixture changes but identified the ancestor-opacity risk; c6b corrected it, with a CSS regression and actual computed-style browser verification.

## Evidence and remaining gates

Phoenix harness-local evidence is preserved under `.phoenix/qualification/` in the feature worktree: original e301 screenshots/failures and 32 assertions; `theme-correction-c6b/` TypeScript/test logs; `theme-correction-render/` eight-case assertions and 36 screenshots; `theme-render-slot/` resource/cleanup receipts. The final render took 16.133 seconds; its owned process group and port62107 listener were independently absent afterward. These local pointers are not portable CI artifacts.

At browser qualification, full workspace codegen, full suite/hosted checks, and current-main integration review remained open. The local codegen guard interruption was not a successful full codegen run; required individual exports were genuine. The hosted follow-up below supersedes that codegen execution status. No compiler or installation was used for final rendered acceptance. The contextual inline-note and Codex draft-display prototypes are separate and are not part of this feature or its acceptance evidence.

## Hosted qualification follow-up

Hosted run `38733795185` at a1fbb6e completed full codegen and full cargo tests successfully, as well as TypeScript, ESLint, full Vitest, spec shape/anchors and task validation. Its failing lanes were: stale generated SseWireEvent documentation, Rust formatting, Clippy (three wildcard enum matches, one needless owned argument and an unreadable test literal), and nine CSS stylelint diagnostics. These were branch-introduced qualification defects, not evidence of a stale-base runtime failure.

Bounded repairs enumerate existing enum variants (future variants require an explicit choice), borrow the wire serializer input, format the affected Rust/CSS, and regenerate SseWireEvent from the verified existing ts-rs test binary. The genuine export ran exactly one named test with 1 pass/1937 filtered; its only generated change is the two-line documentation comment describing explicit projection instead of implicit From. No generated code was hand-edited and no local compiler was launched. Rustfmt checks passed. A focused stylelint run exposed missing inter-rule blank lines after the first formatting edit; those were corrected, with final confirmation left to the next hosted run rather than silently rerunning a consumed admission.

The full-codegen execution gate therefore has hosted evidence, but generated-file freshness and final green Rust/Clippy/UI lanes remain pending the repair head. The 88 rendered assertions remain scoped to the previously qualified presentation behavior; repairs do not constitute a new full browser or physical-phone campaign.

### Local captured-fact privacy correction (source-only)

Independent review of the local typed-watch freeze found that saved goal JSON in
normal diagnostic text bypassed shared presentation redaction. The follow-up
source restores the legacy notification text unchanged. Saved facts remain in
authenticated typed presentation and are loaded separately by actual executor
prompt assembly into ephemeral user-message context before budgeting/freeze.
Nothing adds them to serialized message content, display data or `llm_text`.

Authored **UNRUN** regressions admit a genuine captured blocked-goal notification
through the send service, materialize the accepted payload through its canonical
repository boundary, and replay the same delivery. Shared HTTP initial/history
and SSE initial/pending/live/replay wires must contain no unique saved objective,
reason or goal ID anywhere; original diagnostics remain unchanged. Authenticated
presentation and actual normal/continuation LLM request assertions retain the
facts. Missing/corrupt expected capture refuses context construction, separately
from existing clear-watermark fallback. Source deletion and replay use immutable
event facts, not live goals. These are source assertions, not runtime qualification;
generated exports and all parent-owned hosted capture/qualification gates remain.
