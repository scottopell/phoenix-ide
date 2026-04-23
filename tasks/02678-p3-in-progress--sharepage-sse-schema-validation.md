---
created: 2026-04-23
priority: p3
status: in-progress
artifact: ui/src/pages/SharePage.tsx
---

# Apply SSE schema validation to SharePage

## Problem

Task 02674 (commit `59c5ef0`) added schema validation at every SSE
`addEventListener` in `ui/src/hooks/useConnection.ts`. `SharePage.tsx`
has four more parse sites that still use the pre-02674 pattern:

- `SharePage.tsx:82` — `init` — `JSON.parse(...) as SseInitData`
- `SharePage.tsx:92` — `message` — `JSON.parse(...) as SseMessageData`
- `SharePage.tsx:101` — `state_change` — `JSON.parse(...) as SseStateChangeData`
- `SharePage.tsx:110` — `token` — `JSON.parse(...) as { text: string; request_id?: string }`

These were legitimately out of scope for 02674 (its `artifact:` frontmatter
pointed at `useConnection.ts` only), but the same class of silent drift
exists here: a malformed server payload reaches the SharePage's local
state without runtime validation.

## Design

Reuse the `parseEvent` helper and schemas already exported from 02674:

- `parseEvent` is exported from `ui/src/hooks/useConnection.ts`
- Schemas are exported from `ui/src/sseSchemas.ts`

Each parse site in SharePage becomes:

```ts
es.addEventListener('init', (e) => {
  const res = parseEvent(SseInitDataSchema, e, 'init', dispatch);
  if (!res.ok) return;
  // ... use res.data
});
```

SharePage's dispatch target is its local reducer — not the conversation
atom. The `parseEvent` helper takes the dispatch function as a
parameter, so this works uniformly. The prod failure mode
(`sse_error`) will land in SharePage's local state rather than the
global atom, which is the correct shape — share pages have their own
error surface.

## Acceptance Criteria

- [ ] All four parse sites in `SharePage.tsx` use `parseEvent`.
- [ ] No `as SseInitData` / `as SseMessageData` etc. at parse sites.
- [ ] SharePage's token handler uses `SseTokenDataSchema` instead of
      the inline ad-hoc type `{ text: string; request_id?: string }`.
- [ ] SharePage's local error state handles a `sse_error` action (or
      its equivalent) dispatched by the validator's prod failure path.
- [ ] Tests for SharePage's parse paths (or at least a smoke test that
      confirms malformed input doesn't reach the reducer).
- [ ] `./dev.py check` passes.

## Dependencies

02674 must be merged (it is, as of `59c5ef0` + `fdcbf9e`).

## Out of Scope

- Further SSE hardening (see 02675 for unified sequence-id dedup,
  02676 for derive-don't-patch user messages).
