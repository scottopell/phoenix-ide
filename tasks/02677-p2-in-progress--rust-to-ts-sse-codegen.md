---
created: 2026-04-23
priority: p2
status: in-progress
artifact: src/api/sse.rs
---

# Rust → TS codegen for SSE event types

## Problem

SSE event types are defined on the Rust side (`src/runtime.rs:152` —
`pub enum SseEvent`) and consumed on the TypeScript side
(`ui/src/api.ts` — `SseInitData`, `SseMessageData`, etc.). Today these
are maintained by hand on both sides with zero automated sync:

- `src/api/sse.rs:38-146` constructs wire JSON with `serde_json::json!(...)`
  macros rather than deriving `Serialize` on a typed struct. The wire
  shape exists only as the sum of those macros.
- `ui/src/api.ts` has hand-authored type declarations that devs must
  keep in sync with the Rust side by convention.
- Task 02674 (SSE schema validation) will add hand-authored zod/valibot
  schemas — a third hand-maintained representation of the same shapes.

Three-way drift risk:

```
Rust wire shape  ─┐
                  ├── manually kept in sync
TS type (api.ts) ─┤── manually kept in sync
                  │
Zod schema ──────┘
```

Every field rename/addition is three edits in two languages. TSC catches
TS-vs-zod drift if types are derived from schemas; nothing catches
Rust-vs-TS drift until it manifests at runtime (and then only if 02674
has landed).

## Design

### Phase 1 — Rust-side cleanup (prerequisite)

Replace the `json!()` macros in `src/api/sse.rs:38-146` with a serde-derived
wire enum. The enum already exists (`SseEvent`) but can't be directly
serialized because:

1. Two variants (`Init.messages`, `Message.message`) go through
   `enrich_message_for_api` to transform `db::Message` → enriched
   `Value`. This transformation can't live in `#[derive(Serialize)]`.
2. The existing format is internally tagged with `type` as the
   discriminator, which maps cleanly to `#[serde(tag = "type", rename_all = "snake_case")]`.

Design: introduce an `SseWireEvent` (or similar) enum that holds
already-enriched values. `sse_event_to_axum` becomes
`SseEvent -> SseWireEvent -> serde_json::to_value`. The enrichment step
stays where it is — it becomes a `From<SseEvent> for SseWireEvent`
conversion or a method.

Sample shape:

```rust
#[derive(Serialize, /* + codegen derive */)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseWireEvent {
    Init {
        conversation: EnrichedConversation,
        messages: Vec<EnrichedMessage>,
        agent_working: bool,
        display_state: String,
        last_sequence_id: i64,
        context_window_size: u64,
        model_context_window: usize,
        breadcrumbs: Vec<SseBreadcrumb>,
        commits_behind: u32,
        commits_ahead: u32,
        project_name: Option<String>,
    },
    Message { message: EnrichedMessage },
    MessageUpdated { message_id: String, display_data: Option<Value>, content: Option<MessageContent> },
    // ... etc.
}
```

This refactor is independently worthwhile — it brings the SSE wire
shape into the Rust type system. New fields become compile-time checked;
invariants are enforced by types; `clippy` catches unused variants.

### Phase 2 — Pick a codegen tool

Three realistic options. Evaluator picks one based on appetite.

| Tool | Emits | Maturity | Notes |
|---|---|---|---|
| **ts-rs v12** | TS types only | stable (2019-) | Derive macro + `cargo test` emits `.d.ts`. Mature, narrow scope. Used by e.g. atuin, Warp. |
| **schemars + json-schema-to-zod** | JSON Schema → TS + zod | both pieces mature | Two-step pipeline. Intermediate JSON Schema is a real cross-language contract. Emits both TS types AND zod schemas. |
| **specta v2** | TS types (+ zod WIP) | pre-1.0 RC | `specta-zod` is `0.0.2` and self-describes as "not yet ready for general use." Skip for now. |

**Default recommendation: ts-rs.** Smallest surface area, proven, fits
the "small number of SSE variant types" scope. Leaves 02674's zod
schemas hand-authored, but TSC enforces that the schemas' inferred
output types match the ts-rs-generated Rust-derived types — so drift
surfaces at compile time instead of runtime.

**If appetite is higher: schemars + json-schema-to-zod.** Closes the
full loop (Rust → schemas → both TS types and zod schemas from one
source). Heavier tooling (two generators, JSON Schema as intermediate
artifact). Worth it if we want the zod schemas to also be derived.

### Phase 3 — Build / CI integration

- Generated files live in a dedicated directory (e.g. `ui/src/generated/sse.ts`).
- Codegen runs via `cargo test --features codegen` (ts-rs) or a dedicated
  `cargo xtask codegen` binary (schemars route).
- CI check: regenerate, then `git diff --exit-code` on the generated path.
  Stale generated files fail the build.
- `./dev.py check` runs the codegen-stale check.
- `./dev.py up` / `restart` do NOT need to regenerate on every hot-reload
  (generated TS types are checked into git; Vite consumes them as
  normal source).

### Phase 4 — Migrate 02674's hand-authored schemas (if ts-rs route)

After ts-rs lands:

- Delete the hand-authored TS type declarations from `ui/src/api.ts`
  that are now generated.
- Update zod schemas in 02674 to declare themselves as matching a
  ts-rs-generated type: e.g.

  ```ts
  import type { SseInitData } from '../generated/sse';
  const SseInitSchema: Schema<SseInitData> = v.object({ ... });
  ```

  TSC catches drift between schema and type.

## Acceptance Criteria

- [ ] `src/api/sse.rs` no longer uses `json!()` macros. `SseWireEvent`
      (or equivalent) is serde-derived with `tag = "type"` internal
      tagging. `sse_event_to_axum` becomes a typed conversion +
      `serde_json::to_value`.
- [ ] Every existing SSE wire-shape is preserved byte-for-byte —
      verified by an integration test that captures a known-good JSON
      payload for each variant and asserts the new typed path produces
      the same output.
- [ ] A codegen tool (ts-rs or equivalent) is wired in. Generated TS
      types land in `ui/src/generated/` (or similar).
- [ ] `./dev.py check` has a stale-generated-files check (regenerate
      + `git diff --exit-code`) that fails the build if a dev changed
      a Rust type without regenerating.
- [ ] Task 02674's hand-authored TS type declarations are deleted or
      replaced with re-exports from the generated path.
- [ ] Task 02674's zod schemas are typed against the generated types
      so TSC enforces schema-type alignment.
- [ ] Developer docs (AGENTS.md) note the codegen step and how to
      regenerate.

## Dependencies

- Task 02675 should land first. 02675 adds `sequence_id` to every SSE
  event variant (server-side) — which means the Phase 1 Rust refactor
  here will touch more variants if done after 02675. Better to
  refactor `sse.rs` once against the full post-02675 shape than twice.
  The runtime-validation layer from 02674 is already in place; this
  task is about making those schemas derivable rather than
  hand-authored.
- No server-side wire-format changes in this task beyond what 02675
  already shipped — Phase 1 preserves the 02675 wire shape.

## Out of Scope

- Non-SSE API types (request/response bodies for `POST /api/...`
  endpoints). Worth doing eventually but not in this task's scope.
- Server-side API documentation (OpenAPI/utoipa). Adjacent concern.
- Database schema / migration types.
- Error type shapes beyond what `SseEvent::Error` carries.

## Rationale

Eliminates the three-way drift risk between Rust wire format, TS
types, and zod schemas. After 02674 lands, drift is detectable at
runtime. After this task lands, drift is detectable at compile time
(or, with the schemars route, prevented entirely by construction).

Independent win: the Phase 1 Rust refactor replaces a ~110-line
`json!()` block with typed serde-derived code — simpler, safer,
easier to modify.

## Spike findings (context for the implementer)

ts-rs @ v12.0.0 (Jan 2026): mature, `#[derive(TS)]` + `#[ts(export)]`,
supports serde `tag`/`rename_all` via `serde-compat` feature. Emits
via `cargo test`.

specta @ v2.0.0-rc.24 (Mar 2026): still in RC after ~2 years of RCs.
`specta-typescript` at `0.0.11`, `specta-zod` at `0.0.2` with a
warning "not yet ready for general purpose use" in its lib.rs header.
Defer.

schemars (mature, widely used in OpenAPI tooling) +
json-schema-to-zod (mature npm package): two-step pipeline via JSON
Schema intermediate. More tooling but full-loop codegen.
