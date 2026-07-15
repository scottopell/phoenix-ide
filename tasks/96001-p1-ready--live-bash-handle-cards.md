# Make every bash handle card inspectable and live-tail in-flight waits

## Problem

The conversation transcript exposes `inspect →` only after a bash tool result has arrived with a structured `handle` field. That works for the original `op="run"` result, but it misses the moment when inspection is most useful: a later `op="wait"`, `op="peek"`, or `op="kill"` call already identifies its handle in the tool input while the call is still in flight, yet its card has neither an inspect affordance nor any live output. The user must find the earlier run card and open its inspector to see progress.

The full process inspector already provides a polling live tail plus resource metrics. Automatically using that combined endpoint for every visible in-flight card would unnecessarily sample CPU, proportional memory, and process count once per second. Inline progress needs the same authoritative ring cursor, but not the health sampler.

## Goal

Make handle identity a first-class property of every valid handle operation in the transcript, and show bounded live output directly in an in-flight handle-operation card while preserving the full inspector as the explicit detailed health view.

## Plan

### 1. Specify the two transcript affordances

Update the timeless process-inspector/conversation UI requirements and executive status to require:

- every valid handle-bearing bash tool card to expose `inspect →` as soon as both the work-scope key and handle id are known;
- `run` cards to obtain the handle from their result (the input cannot know it before the server allocates it);
- `peek`, `wait`, and `kill` cards to obtain the handle from their typed input while in flight and from the typed response after completion;
- failed/invalid calls and handle-not-found results not to advertise an inspector target that cannot resolve;
- an in-flight handle operation to show a bounded, incrementally updated output tail inline;
- the detailed inspector to remain the opt-in surface for identity and resource metrics.

The process inspector remains a read projection rather than a new lifecycle state machine; no new Allium spec is expected. Run the spec-authoring pre-flight checklist before completion.

### 2. Add an output-only handle read surface

Add a typed, read-only work-scope endpoint for a handle's `BashRingWindow`, keyed by `(scope_key, handle_id)` and accepting the existing optional `since` cursor. Reuse the authoritative registry lookup and ring/tombstone read helpers already used by `assemble_inspection` and bash `peek`.

This endpoint must:

- return the same complete-line offsets, trailing `partial`, `truncated_before`, and bounded ring/tombstone semantics as the inspector;
- return not-found for an absent scope/handle;
- perform no process/resource sampling and introduce no persistence;
- avoid creating a handle table on reads;
- expose generated TypeScript types through the existing Rust codegen path rather than adding a parallel handwritten wire shape.

Refactor the existing projection only as needed to share the output read without duplicating ring semantics.

### 3. Resolve inspectability from typed input and output

In `MessageComponents.tsx`, introduce one bash-handle resolver for tool cards:

- validate modern `BashToolInput` and read `input.handle` for `peek`/`wait`/`kill`;
- prefer/confirm the structured completed response handle where available;
- suppress the target for errors such as `handle_not_found` rather than retaining a stale input fallback;
- keep legacy/plain bash results and contexts without `workScopeKey` safe.

Render one `inspect →` affordance at the tool-card level so it is available before a result exists and is not duplicated inside the completed response body. Continue isolating the viewer-slot hook in a conditional child component so render paths without a `ViewerSlotProvider` do not throw.

### 4. Live-tail in-flight handle operations inline

While a valid `peek`, `wait`, or `kill` card is in the `running` tool-card state, poll the output-only endpoint at roughly the inspector cadence using `since = prior end_offset`.

The inline tail should:

- seed with a recent bounded tail, append deltas without duplicate offsets, and render a live trailing `partial` distinctly;
- show an inline truncation/gap marker when older output has fallen out of the ring;
- cap retained/rendered entries so a long-running command cannot grow the conversation DOM without bound;
- follow the newest output by default without hijacking the conversation's outer scroll position;
- stop polling immediately when the tool result arrives, the component unmounts, the target changes, or the handle is definitively gone;
- retain the ordinary completed `BashResponseView` once the tool result arrives, with no competing polling loop;
- expose transient stale/error state compactly without replacing already observed output.

Share the inspector's cursor/deduplication logic where that reduces race risk, but keep resource fetching exclusive to the full inspector. Ensure at most one request is in flight per inline tail and guard target changes against stale responses.

### 5. Verification

Add focused coverage for:

- an in-flight `wait` card immediately renders `inspect →` from `input.handle` and opens the correct `(work_scope_key, handle_id)` URL;
- in-flight `peek` and `kill` cards receive the same affordance;
- a completed original `run` result remains inspectable from its response handle;
- no work-scope key, malformed/legacy input without a response handle, and `handle_not_found` do not render a broken affordance;
- the inline tail seeds, appends incremental lines exactly once, replaces partial output, marks truncation, bounds retained entries, and stops on completion/unmount/404;
- overlapping polls are prevented and a stale response from a prior target cannot mutate the new target;
- the output-only backend route reads live and tombstoned windows, preserves offset semantics, returns 404 appropriately, and never invokes resource sampling;
- generated UI types are current and relevant UI/Rust tests plus `./dev.py check` pass.

## Acceptance criteria

- In the screenshot journey, the visible in-flight `wait b-41` card itself offers `inspect →`; the user no longer has to locate the original run card.
- That wait card displays fresh command output inline while it is waiting.
- Every valid `peek`/`wait`/`kill` transcript card is inspectable as soon as its input renders, and every structured `run` result carrying a handle remains inspectable.
- Inline tailing is bounded, cursor-correct, and stops with the card/tool lifecycle.
- Automatic inline tailing does not trigger CPU/memory/process resource sampling; those metrics remain exclusive to the explicitly opened inspector.
