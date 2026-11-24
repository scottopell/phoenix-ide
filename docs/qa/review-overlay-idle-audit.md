# Image-free Review message idle-scroll audit

**Scope qualification:** this document records short, static browser windows, not an exhausted code-path investigation. See [the subsequent source-first audit](review-overlay-code-path-audit.md) for concrete delayed candidates, lifecycle guards, and corrections to the initial source inventory.

## Outcome

No settled/no-input phantom scroll reproduced in the bounded hosted Chromium audit. No product correction is justified by this evidence. This is not physical iPhone/Safari/PWA acceptance.

The surface audited is `MessageReviewAction` → `MessageViewer` → `ViewerShell`, not the compact draft-selection annotation dialog. The parallel image-loading transcript investigation is a separate symptom and owner.

## Exact evidence

- Product baseline: `d3b57573ccf8662d05125338aad2307bb03da598`.
- Qualified fixture/probe commit: `10e38604b8477e47e36605cc2877ab18c9d1a3ec`, branch `investigate/annotation-overlay-idle`.
- [Hosted run 43894589684](https://github.com/scottopell/phoenix-ide/actions/runs/43894589684).
- [Successful geometry job 133395037653](https://github.com/scottopell/phoenix-ide/actions/runs/43894589684/job/133395037653).
- [Artifact 14238478648](https://github.com/scottopell/phoenix-ide/actions/runs/43894589684/artifacts/14238478648), named `assistant-finalization-geometry-10e38604b8477e47e36605cc2877ab18c9d1a3ec`.
- Artifact report: `review-overlay-idle/report.json`; screenshots alongside it. Mobile editor and post-navigation screenshots inspected, as were earlier mobile review-open and desktop note-navigation captures.
- Existing 42-case reading-position matrix also passed with zero mismatches. Hosted UI/specs and e2e lanes passed. Subsequent GitHub status verification confirmed the entire run completed with conclusion `success` at the exact qualified probe commit.
- Local validation was limited to `node --check` and `git diff --check`. No dependencies installed, local native builds, production mutation, restart, or deploy.

## Probe and results

Existing production conversation fixtures were extended with long, image-free review text. The existing browser capture script and hosted geometry lane were reused; no new harness or dependency. Chromium used 390×844 touch-enabled and 1440×900 desktop contexts, for ordinary and global conversation routes.

Each of four cases recorded four 2400ms intervals after setup settled:

| Interval | Cases | Result |
|---|---:|---|
| Review open, middle of long message | 4 | No motion |
| Annotation editor open with filled text | 4 | No motion |
| Saved note | 4 | No motion |
| Following explicit note navigation, spanning highlight expiry | 4 | No motion; highlight active → inactive verified |

All 16 intervals recorded 145 frame samples each. Maximum paragraph-top displacement was **0 px**; text Range rectangles, scroll offsets/geometry, document/visual viewport, and focus stayed unchanged. No idle scroll/focus/input events or wrapped focus/scroll commands occurred. In all four expiry intervals the visible paragraph node changed identity, but its text and geometry did not move. DOM remount alone is not phantom scrolling.

The probe observes `.viewer-content`, `.viewer-shell`, `#messages`, the document scrolling element, and viewer-content ancestors independently. It records command stacks and setup events separately from idle events. Default reading-position journeys remain enabled. Mobile annotation setup uses an actual long press near viewport center, not the hidden hover-only Add Note control.

## Initial conditional source inventory

This inventory records the browser-audit phase; the subsequent [source-first audit](review-overlay-code-path-audit.md) supersedes its prioritization and adds background projection/Mermaid, clipboard and sibling-focus paths.

1. **Annotation editor autofocus.** `AnnotationDialog` focuses its textarea on mount without `preventScroll`. Native focus/keyboard reveal can affect ancestors or the visual viewport. Requires mount/remount or a related platform viewport event; no recurring idle focus timer was found. Highest-value next path if physical motion follows editor/keyboard activity. No generic focus suppression is warranted.
2. **Saved-note navigation and highlight expiry.** `MessageViewer.handleJumpTo` calls line `scrollIntoView({behavior: 'smooth', block: 'center'})` only after explicit navigation. Its note hook clears highlight after two seconds. The memoized Markdown component map can remount annotatable blocks when highlight changes. The audit observed expiry/remount with no motion in all four cases, weakening this path for these conditions without excluding Safari-specific behavior.
3. **Viewer presentation/focus transitions.** `ViewerShell`'s fallback portal move restores focused descendants with `preventScroll` and descendant pixel offsets; it does not ordinarily autofocus on mount or implement a focus trap. Ancestor offsets and semantic text positions are not restored. Actual mode/destination changes are conditional triggers; no steady idle trigger was found.
4. **Underlying transcript or a hidden ancestor.** Updates may continue behind an open viewer. Hidden overflow does not make ancestors programmatically unscrollable. Determine which layer moved before assigning a cause; the separate transcript owner retains that investigation.
5. **Review-note store updates.** Correction from the subsequent source-first audit: `ReviewNotesContext` is a synchronous **in-memory** external store, not session-storage hydration. Scope changes create a new empty store. There is no notes fetch/polling/restore timer capable of opening the annotation editor.
6. **Long press.** `useLongPress` requires a preceding touch/mouse start and a 500ms hold, with movement/release/unmount cancellation. Subsequent source inspection found a concrete gap: `AnnotatableBlock` does **not** bind `touchcancel`, so cancellation without a handled release/movement can leave an earlier hold pending. It is not a timer under strictly no preceding input.

The five viewer components (`MessageViewer`, `ViewerShell`, `MarkdownViewerBody`, `AnnotationDialog`, `NotesPanel`) and the viewer directory were unchanged between RC6 `bccfd66b2470a86910b952f22ccf327ed2c78f2f` and the product baseline. The `index.css` delta was an unrelated transcript-table width rule. This is source comparison, not a browser qualification of RC6 or proof of identical surrounding runtime/platform state.

## Failed attempts and adjacent observation

Earlier attempts are retained as diagnostic evidence, not passes:

- Run `43890751138`: desktop idle journeys passed, but mobile editing attempted a hover-only control hidden by touch CSS. This was a probe error.
- Run `43892745662`: actual long press near the top opened the editor, but it disappeared after release before filling. Desktop journeys and mobile review-open intervals stayed stable. Source permits a touch-generated click on the newly mounted annotation backdrop to cancel it. The centered-touch final run recorded a release-generated click on the textarea and retained the editor. This supports investigating a position-dependent touch-release targeting boundary, but the failing run lacked click-target telemetry: backdrop dismissal is **not proved**, and is not an idle-scroll finding.

Follow-up TODO: reproduce the near-top long-press release with event-target/coordinate and post-click editor-presence telemetry if investigating annotation dismissal. Do not silently broaden this audit into a product patch or claim physical iOS reproduction.

## Limits and smallest discriminating next observation

Only short settled intervals were sampled, once per condition. The probe does not emulate the iOS keyboard, PWA viewport lifecycle, browser chrome changes, long idle periods, live background traffic, or font/platform-specific relayout. Wrapped commands do not cover direct `scrollTop` assignment or every Window API; measured offsets still detect sustained movement. Between-frame transient changes cannot be universally excluded.

The next source-backed discriminator is not another static idle wait: hold selected Markdown constant, publish a qualifying background projection, and check renderer identity plus geometry of an already-rendered Mermaid above the reading point. The [source-first audit](review-overlay-code-path-audit.md) specifies prerequisites and alternatives. A naturally occurring iPhone event can additionally distinguish text moving inside Review message, the whole overlay moving, or the transcript behind it moving, but investigation does not depend on obtaining a new capture. No production mutation or private-capture modification is required.
