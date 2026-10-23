# Touch reading-position prototype — SOURCE ONLY, STREAMING BLOCKED

No tests, typecheck, builds, servers, browser sessions, measurements, commits or
pushes were run for this correction. These files are proposed experiment source,
not runtime evidence. Changes are confined to the new runner and this new directory.
No shared renderer, store, fixture API, controller, dependency or config was changed.

## Exact streaming acceptance blocker

`ProductConversationFixture` privately mounts `ConversationProvider` inside
`ProductConversationFixtureBody`. Its public props expose a scenario, not children
or a descendant observer slot. The local `TouchReadingFixture` wraps that provider;
it cannot read its context/store using a hook outside the provider. Inspection of
`ConversationStore.dispatch` and `atom.ts` confirms that actual token acceptance is
represented by the reduced `streamingBuffer` (`requestId`, `lastSequence`, `text`),
not by an EventSource producer callback returning. Tokens can be rejected by phase.

**Streaming acceptance is BLOCKED.** The existing local EventSource Proxy observes
listener registration and emits wire events; its `status().tokenText` and `emitted`
are producer bookkeeping ONLY. Neither these counters, listener invocation, an
initial visible marker, nor a final accumulated render prove in-window production
store consumption. No acknowledgement is fabricated from those signals.

The narrowly needed follow-up is an authorized optional fixture observer slot
**under the existing ConversationProvider**, allowing a local hook to subscribe to
that actual ConversationStore. No second provider/store and no prototype patching
of global store prototypes should stand in for it. Such a shared change is outside
this source-only scope and has NOT been made. Once available, acceptance must bind
request/sequence/exact accumulated text to real store snapshots during each of
`drag`, `hold`, `release_settle`, and `post_release`, and separately demonstrate the
actual workload/layout (including real SSE reflow geometry where prescribed).
Store consumption alone would not prove rendering or layout work occurred.

The runner therefore marks streaming acceptance `BLOCKED`, returns a `blocked`
case before attempting either positive streaming case, and exits nonzero even if
all preceding geometric diagnostics complete. Final-render inspection is only
supplemental code, unreachable behind this blocker; it cannot qualify a pass.
Unreached cases remain explicit `unattempted` records, including after early errors.

## Production path and remaining integration uncertainty

The scenario supplies 36 variable-height persisted assistant messages in a writable
member through the unchanged ProductConversationFixture. That mounts the actual
ProductConversationPage, ConversationProvider, MessageList/VirtualTranscript and
StreamingMessage. The fixture API's EventSource emitter is private; the local parent
passive-effect Proxy taps registered production listeners after child installation
but before its body mounts. That ordering, wire acceptance and subscription coverage
remain unexecuted and untypechecked. A failed initial marker/real reflow is a failure,
not permission to substitute DOM streaming text. The initial marker is not a store
acknowledgement. The standalone entry adds no StrictMode replay or service worker.

## Identity and geometric oracle

Positioning uses Chromium CDP touch pan, slow continued drag, hold, and release —
not wheel, scrollIntoView, synthetic touch or setup scrollTop writes. Acquisition
requires trusted movement and >=24px native scroll. The single deliberate scrollTop
write is the labelled forced-bottom control.

The anchor is transcript ID + exact message occurrence + render-unit key + stable
production `data-fragment-id` content-block ID + UTF-16 logical text offset across
that block's text nodes, full block text and surrounding quote. Every frame finds
exactly one occurrence and exactly one block before resolving that logical offset
back to the current DOM node/Range. Node ordinal is NOT identity. Equivalent text
node splitting/merging or wrapper resegmentation preserves identity; duplicate
occurrences/blocks, altered text, missing glyph geometry or absent units fail closed.
No nearest-row or approximate quote fallback is allowed. The acquired persisted
passage never becomes the streaming tail.

`semanticAnchor.mjs` supplies the same pure resolver installed into the browser.
`oracle.test.mjs` contains source assertions for split/merged/empty segments,
wrong occurrence, duplicate blocks and altered content. These fixtures have NOT
been run and do not claim browser remount or resegmentation evidence.

rAF samples glyph position relative to the acquired finger offset, scroll extent,
layout revision, mounted units, range/message/unit geometry and missing state.
Max displacement is checked separately for drag, hold, release-settle and released
reading; no averaging and no release rebasing. Missing observations fail, even if
a later frame recovers. Fixed calibration ceiling is 6px; successful calibration
freezes `max(2, ceil(maxBaselineDisplacement)+2)`, at most 8px. Never widen on failure.

## Corrected case ordering and causal controls

The bounded order is:

1. `no-stream`: inject the SAME `TOUCH_STREAM_SEED_1729.\n\n` initial tail marker as
   every other case, then schedule NO growth. Check exactly one producer event and
   unchanged producer text. This aligns initial renderer state, not consumption proof.
2. `forced-scroll-bottom`: require >=48px physical write and displacement or exact
   occurrence unmount within the immediate causal window.
3. `uncompensated-above-height`: insert a labelled empty 160px span above the grabbed
   text; require displacement, NOT merely a missing anchor.
4. `oracle-anchor-invalid`: invalidate only the resolver; require its specific
   deliberate observability failure, not arbitrary loss of production text.
5. `stream-chunks`: BLOCKED before positive execution.
6. `stream-burst-reflow`: unattempted after that blocker (also requires the store seam).

Control producers stop at 300ms, then remain quiescent for 700ms before the planned
1000ms injection. Controls schedule no real message reflow and no later tokens.
The scheduler explicitly pauses and awaits `control-injection` → `control-ack`:
exactly four sampled rAF opportunities, at most 200ms. Here **ack means end of the
control observation window ONLY**, not store consumption. Missing/slow/empty or
oversized windows fail. All pre-injection measured frames must be clean, with at
least two samples. Detection is taken only from the indexed, hold-phase window;
a later reflow/release failure cannot earn detection. Any scheduled perturbation
inside it invalidates the control. Pure source assertions cover late-only failure,
confounded windows, timeout, pre-existing failure and specific invalidation.
Controls compensated successfully are not detection proof.

## Future execution only after separate authority

No execution is authorized by this README. Once host/testing admission is separately
granted, the pure source assertions can be run with:

```sh
node ui/src/fixtures/touchReading/oracle.test.mjs
```

The browser runner requires an already-running, separately admitted loopback Vite
dev server for THIS worktree, existing Playwright/Chromium, a new artifact directory,
`--admitted-host` and `PHOENIX_TOUCH_HOST_ADMITTED=1`. It never installs dependencies,
starts servers or builds. Example for future diagnostic execution, which will still
exit nonzero because streaming acceptance is blocked:

```sh
PHOENIX_TOUCH_HOST_ADMITTED=1 \
TOUCH_FIXTURE_URL=http://127.0.0.1:ADMITTED_PORT/src/fixtures/touchReading/index.html \
TOUCH_QA_OUT=/ABSOLUTE/NEW/ARTIFACT_DIRECTORY \
node ui/scripts/capture-touch-reading-position.mjs --admitted-host
```

One 390x844 DPR1 viewport, seed1729, one browser, sequential cases, no retries,
15s launch timeout and 240s deadline. Proposed bounds are not measured runtime.
Attempted diagnostics capture video, screenshots and trace JSON; summary lists all
case statuses. Limits remain 2400 frames, 512 events, 16 bounded page errors,
64 detailed violations plus total count, and 2MB per JSON; overflow fails closed.
No artifacts exist from this source-only correction.

## Limits

Main-thread rAF/Range is not a compositor-frame oracle; video is review evidence,
not automated sub-frame proof. CDP Chromium touch is not physical input, Safari,
WKWebView, iPhone or installed-PWA coverage. One seed/viewport/baseline and pure
resegmentation assertions do not cover production remount histories. No production
controller behavior was changed or qualified. All runtime validation remains pending;
streaming acceptance additionally needs the exact descendant store observer seam above.
