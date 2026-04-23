---
created: 2026-04-23
priority: p2
status: in-progress
artifact: ui/src/hooks/useConnection.ts
---

# SSE wire-format validation at the parse boundary

## Problem

Every SSE event handler in `ui/src/hooks/useConnection.ts` follows the same pattern:

```ts
let data: SseMessageData;
try {
  data = JSON.parse((e as MessageEvent).data) as SseMessageData;
} catch { ... }
```

The `as SseMessageData` is a TypeScript assertion with zero runtime enforcement.
If the server ever sends a malformed-but-parseable payload (missing field, wrong
type, null where a string was expected), the reducer consumes garbage silently.

Concrete risk: if `sequence_id` arrives as a string instead of a number, the
reducer guard `atom.lastSequenceId >= action.sequenceId` becomes
`0 >= "101"` → `false` (string comparison) → the message is appended with a
string `sequence_id`, `atom.lastSequenceId` becomes a string, and future
dedup is broken for every subsequent event. No warning, no error, no way
to notice until the UI diverges from server truth.

Parse-failure handling is also inconsistent across event types:

- Most handlers dispatch `sse_error` on JSON parse failure.
- `token` events silently `return` (`useConnection.ts:259-260`) with the
  comment "non-fatal — ephemeral events, skip silently." A persistent parse
  failure on tokens means LLM output silently stops streaming with no
  user-visible signal.

## Design

Add runtime schema validation at each `addEventListener` in `useConnection.ts`.

**Library choice**: valibot (smaller bundle, same DX) or zod (more ecosystem
familiarity). Match whatever the codebase already uses if present; otherwise
valibot for size.

Co-locate schemas with the SSE event types. Derive TypeScript types from
schemas so the wire contract is a single source of truth:

```ts
const SseMessageDataSchema = v.object({
  message: v.object({
    message_id: v.string(),
    sequence_id: v.number(),
    ...
  }),
});
export type SseMessageData = v.InferOutput<typeof SseMessageDataSchema>;
```

### Failure modes

- **Dev (`import.meta.env.DEV`)**: `console.error` with structured detail
  (event type, raw payload, schema violation path), then throw so the
  React error boundary catches it. Loud — contract drift is a bug, not
  a warning.
- **Prod**: dispatch `sse_error` with a structured violation message;
  UI must not crash. If telemetry exists, emit there too.

### Handler call site shape

Extract a helper to keep sites short:

```ts
function parseEvent<T>(schema: Schema<T>, e: Event, type: string): T | null {
  const raw = (e as MessageEvent).data;
  const parsed = safeParse(schema, JSON.parse(raw));
  if (!parsed.success) {
    handleSchemaViolation(type, raw, parsed.issues);
    return null;
  }
  return parsed.output;
}
```

## Acceptance Criteria

- [ ] Schemas defined for every SSE event type: `init`, `message`,
      `message_updated`, `state_change`, `agent_done`, `token`,
      `conversation_update`, `error`, `conversation_became_terminal`.
- [ ] `ui/src/api.ts` event types derived from schemas (one direction,
      one source of truth).
- [ ] Every `addEventListener` in `useConnection.ts` validates before
      dispatching. No `as <Type>` at the parse site.
- [ ] Dev mode: schema violation throws with structured detail visible
      in the error boundary / console.
- [ ] Prod mode: schema violation dispatches `sse_error` with a
      readable message; UI does not crash.
- [ ] Tests for each handler: malformed payload (missing field,
      wrong type) does not dispatch a corrupt action to the reducer.
- [ ] `sse_token` silent-return is replaced with the same failure
      handling as other events.

## Rationale

Cheapest single change with the highest leverage for SSE fragility.
Turns "server contract drift" from invisible into caught-at-the-boundary.
Every other SSE fragility issue becomes diagnosable instead of
manifesting as "the UI occasionally looks wrong."

## Out of Scope

- Unifying sequence_id dedup across event types (task: `sse-unify-sequence-id-dedup`).
- Deriving rendered state vs. imperative patches (task: `user-message-derived-rendering`).
- Changing the server's wire format.
