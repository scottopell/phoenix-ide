# Known React performance patterns

The React analog of lading's "Known Patterns" table. Each row: a detectable
code pattern, the technique that fixes it, and where it bites. Scan for ALL of
these in every covered component.

| Name | Pattern (what to grep/read for) | Technique |
|------|----------------------------------|-----------|
| `memo-list-item` | List item component re-rendered on every parent render, not wrapped in `React.memo` | Wrap item in `React.memo`, stabilize its props |
| `index-key` | `key={i}` / `key={`...-${i}`}` on a list whose elements reorder/insert | Stable identity key (server id, sequence id) |
| `inline-object-prop` | `prop={{...}}` / `prop={[...]}` passed to a memoized or list child each render | Hoist to module const, `useMemo`, or stable ref |
| `inline-callback-prop` | `onX={() => ...}` passed to a memoized or list child each render | `useCallback` with correct deps, or ref indirection |
| `broad-context` | A Context whose value object is rebuilt every render / changes per token, with many consumers | Split context, memoize value, move volatile state out |
| `unstable-deps` | `useEffect`/`useMemo`/`useCallback` dep array contains a value re-created each render (object/array/fn literal) | Stabilize the dep (memoize the source) |
| `reparse-growing-buffer` | A growing string/buffer fully re-parsed/re-derived on every tick (per-token / per-frame); cost grows with length → O(n²) over the stream | Incremental parse, memoize on a stable cut, parse only the new tail |
| `no-virtualization` | A list of unbounded length rendered fully into the DOM; cost scales with conversation size | Windowing / virtualization for large N |
| `state-too-high` | Volatile high-frequency state (keystroke, token) held in a component whose subtree includes expensive siblings | Lift volatile state into an isolated store/leaf so siblings don't re-render |
| `derive-in-render` | Expensive derivation (parse, sort, filter, JSON) recomputed every render without `useMemo` | `useMemo` keyed on real inputs |
| `effect-cascade` | `useEffect` that calls `setState` causing an immediate second render every tick | Derive during render, or collapse the effect |
| `ref-thrash` | `ResizeObserver`/`IntersectionObserver`/layout read wired to re-run synchronously per token/frame | Debounce / rAF-coalesce / decouple from render |
| `over-subscription` | A component subscribing to a whole store/atom when it needs one slice; re-renders on unrelated changes | Selector subscription (subscribe to the slice only) |

## Hot-path rule

A hit only counts if it is on a path that runs **per token, per keystroke, per
animation frame, or scales with conversation size**. The same pattern off the
hot path is not a target — record it as 0 in the matrix and move on. This
mirrors lading's "verified hot-path hit" requirement: profiling decides where
the path is, the pattern decides what to do about it.
