# Background review projection: remount discriminator

## Scope

A settled Review message can receive an eligible background conversation update while its reviewed message remains unchanged. The investigated chain is projection replacement → aggregate-note callback replacement → annotation-start callback replacement → new Markdown renderer component types → Mermaid remount and loading placeholder. This is a demonstrated hosted Chromium mechanism, not attribution of the reported physical-iPhone/PWA incident.

The bounded correction derives annotation capability as a boolean for `startAnnotate`, retaining `sequenceId` as a dependency. Actual note sending still depends on the latest `onSendNotes` callback. No renderer-wide memoization, focus policy, clipboard change, production operation, native build, or new dependency is involved.

## Pre-fix evidence

- Source: `9ff6e6354e209836de5e83375ca6b27659c80e09`.
- Hosted run: <https://github.com/scottopell/phoenix-ide/actions/runs/43905995735>.
- Geometry job: `133435357728`, successful.
- Artifact: `assistant-finalization-geometry-9ff6e6354e209836de5e83375ca6b27659c80e09`, ID `14246532059`; `review-overlay-projection/report.json` plus before/after PNGs.
- Baseline mode requires the undesired remount and Mermaid loading to occur; it is not the fixed regression gate.
- The same run's UI/spec lane failed on two fixture dataset property accesses under `noPropertyAccessFromIndexSignature`. Indexed accesses correct this fixture-only error in the candidate; the pre-fix geometry result does not imply full CI success.

Each row ran at both widths (390px touch-enabled Chromium and 1440px desktop Chromium). Measurement starts after opening, font/diagram readiness, positioning paragraph 10, and settling. A fixture event then emits a real SSE update through the existing adapter. The mounted conversation-atom probe verifies acceptance, rather than merely recording publication. Roughly 100–109 RAF samples cover the 1.8s measurement window.

| Case | Accepted atom change | Prose remount | Mermaid restarted | Max same-text Y displacement | Final Y displacement |
|---|---|---|---|---:|---:|
| No-update control | None | No | No | 0px | 0px |
| Background projection, Mermaid | Title only; content unchanged | Yes | Yes | 332.40625px | 0px |
| Background projection, text-only | Title only; content unchanged | Yes | N/A | 0px | 0px |
| Intentional reviewed-message change | Reviewed content | Yes | Yes | 298.8125px | 33.59375px |

All eight cases recorded no new input/focus events during measurement. Viewer scrollTop stayed fixed in the Mermaid projection cases (1783px mobile, 1322px desktop): the measured displacement is transient layout collapse/expansion, not evidence of an explicit scroll command. The before/after mobile PNGs were inspected and match; they intentionally do not capture the transient collapse, which is evidenced by RAF samples and mutation observations. Do not interpret matching endpoints as absence of motion.

## Candidate validation

The focused unit regression uses real MessageViewer, MarkdownViewerBody, and react-markdown, mocking only the Mermaid leaf with a mount probe. It replaces an enabled send callback, requires the same leaf DOM/instance, then verifies sending reaches the latest callback. It also checks capability removal/restoration and intentional Markdown changes. Existing message-switch tests cover annotation reset.

The hosted positive browser gate removes `BASELINE=1`. For unchanged content it requires preserved paragraph/diagram/SVG identity, no renewed loading, and at most 1px same-text displacement. It retains no-update, text-only, and intentional-content-change controls. All cases require no new input/focus events. Ancestor offsets and visual viewport are recorded diagnostically, not asserted as a universal guarantee.

Independent read-only review of `03f22ba16c7bc8da7f80013384487b4aac22bf07..7636154cb4018d6a13ddca03bf28a1fe5b0d2eb6` found no actionable defects in the minimal correction, test, or positive gate. Rebase onto `eea16846b1e402de895de9559b54beb9954cf671` preserved these source/test/fixture/script files byte-for-byte; only surrounding main/workflow context advanced. Hosted candidate results and integration evidence must be recorded before claiming completion.
