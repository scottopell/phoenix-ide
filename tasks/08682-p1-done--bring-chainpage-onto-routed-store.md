---
created: 2026-05-05
priority: p1
status: done
artifact: ui/src/pages/ChainPage.tsx
---

# bring-chainpage-onto-routed-store

## Summary

ChainPage is architecturally one generation behind ConversationPage. It uses
plain `useState` for chain data, draft, in-flight Q&A, and submit state, while
ConversationPage extracts equivalent state into `ConversationStore` —
slug-keyed atoms with a `dispatch(slug, action)` API. The asymmetry has
concrete consequences (panel findings, post-02703):

- **Data corruption on navigation.** `ChainPage.refresh` has no cancellation
  guard. Navigating from chain A to chain B while A's Q&A is mid-stream lets
  the resolving `setChain(viewA)` overwrite chain B's component state. The
  same mistake in ConversationPage is structurally harmless because
  `dispatch(oldSlug, action)` lands in a dead atom.
- **Submit isn't actually optimistic.** `submit()` awaits the POST before
  calling `setInflight`, so the textarea is frozen for the round-trip. The
  file-level comment claims optimism; the code disagrees.
- **The synchronous reset block at `ChainPage.tsx:94-105`** is workaround
  infrastructure that exists only because per-component state needs faking
  out as if the component were keyed-remounted. Once state lives in a
  slug-keyed atom, the reset block deletes itself.

## Context

Relevant files:
- `ui/src/pages/ChainPage.tsx` — primary migration target
- `ui/src/conversation/ConversationStore.ts` — pattern source
- `ui/src/hooks/useConnection.ts` — SSE lifecycle precedent (also touched by task B)

## Plan

### Phase 1: Extract or duplicate the routed-store pattern

Decide between:
- **(a)** Generalise `ConversationStore` into a `RoutedStore<K, S>` parameterised
  by key type and atom shape. ConversationStore + ChainStore both consume it.
- **(b)** Duplicate cleanly into a separate `ChainStore` with the same shape.

Recommendation: try (a) first. If the generic types out ugly (likely places:
the action union, the SSE event union), fall back to (b). The two stores
having the same *shape* is more important than DRY.

The store contract:
- `getSnapshot(key) -> Atom | null` — synchronous; returns null until loaded.
- `subscribe(key, listener) -> unsubscribe` — fires when the atom changes.
- `dispatch(key, action)` — routes to the keyed atom; no-op if the atom
  doesn't exist (this is the dead-atom protection).
- React hook: `useChainAtom(key)` returns `{ atom, dispatch }` bound to the key.

### Phase 2: Define ChainAtom

Slots that move from ChainPage `useState` into ChainAtom:
- `chain: ChainView | null`
- `loadError: string | null`
- `loading: boolean`
- `inflight: Record<string, InflightQa>`
- `inflightOrder: string[]`
- `draft: string`
- `submitting: boolean`
- `sseLost: boolean`

(`deleteConfirmOpen` stays component-local — it's a dialog state, not chain
state.)

Action shape mirrors the SSE event union plus user actions:
`LOAD_BEGIN | LOAD_OK | LOAD_FAIL | OPTIMISTIC_INFLIGHT_ADD | TOKEN_APPENDED |
INFLIGHT_RECONCILE | INFLIGHT_FAIL | DRAFT_CHANGED | SUBMIT_BEGIN | SUBMIT_OK |
SUBMIT_FAIL | SSE_LOST | SSE_RESTORED`.

### Phase 3: Migrate ChainPage

- Replace every `useState` slot listed above with reads through
  `useChainAtom(rootConvId!)`.
- Replace every `setX` with a `dispatch({ type: ... })`.
- **Delete the synchronous reset block at lines 94-105.** The store handles
  per-key state by construction; the only component-local state that
  remains (`deleteConfirmOpen`) gets reset via the same in-render pattern
  02703 established or stays local if it's truly per-render-instance.
- **Make `submit()` optimistic.** The flow becomes:
  1. Generate a client-side optimistic id (`crypto.randomUUID()` for a
     local-only handle, OR keep the existing server-issued `chain_qa_id`
     pattern but flip the order — verify what the API contract requires).
  2. `dispatch(rootId, { type: 'OPTIMISTIC_INFLIGHT_ADD', ... })` — synchronous.
  3. Fire the POST.
  4. On success: `dispatch(rootId, { type: 'SUBMIT_OK', ... })` — reconciles
     the optimistic entry with the server-issued ID if different.
  5. On failure: `dispatch(rootId, { type: 'SUBMIT_FAIL', error })`.

  The textarea is no longer disabled during the round-trip. Update the
  file-level comment at `ChainPage.tsx:14-20` to match what the code now does.

### Phase 4: SSE lifecycle

The chain SSE subscription currently lives in ChainPage's `useEffect`. Move it
into the store: when an atom is created (first `useChainAtom(key)` call for
that key), the store opens its EventSource. When no consumers remain, it
closes. This mirrors how ConversationStore handles per-conv connections via
`useConnection`.

If this proves invasive in round one, defer — keep the SSE subscription in
the component for this task, move it into the store as an explicit follow-up.
Note the choice in the PR.

### Phase 5: Tests

- Unit tests for ChainStore action handlers (parity with how
  ConversationStore is tested).
- **Integration test: navigate chain A → chain B mid-stream**, verify
  chain B's page shows chain B's data (not A's), even when A's `getChain`
  resolves after navigation. This is the regression test for the
  data-corruption bug.
- **Test for optimistic submit**: textarea is enabled and contains empty
  string immediately after submit click; in-flight pair card is in the
  DOM before the POST resolves.
- Existing ChainPage tests (572-line file) continue to pass. The
  state-reset test added in 02703 may become a redundancy check on the
  store boundary — keep it as a guard.

## Acceptance Criteria

- ChainPage holds zero `useState` for chain-scoped data; everything reads
  through `useChainAtom`.
- The synchronous reset block (current lines 94-105) is deleted.
- `submit()` updates UI synchronously before the POST; textarea is never
  disabled mid-keystroke.
- Navigating between chains while a Q&A is streaming does not corrupt the
  destination chain's state. New test exercises this scenario.
- File-level comment in ChainPage matches what the code does.
- `./dev.py check` passes.
- No regression in existing chain QA tests.

## Independence

This task can run in parallel with task B (epoch-stamp SSE events). Task C
(single Conversation source of truth) depends on this task landing first.
