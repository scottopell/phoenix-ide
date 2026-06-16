Evaluate migrating the HTTP API request/response types in `crates/phoenix-ide/src/api/types.rs` to `#[derive(ts_rs::TS)]` codegen, the way SSE wire events in `api/wire.rs` already do.

Today these types are plain `Serialize` with no ts_rs derive (see the comment at the top of the response-types block in `types.rs`), and their TypeScript shapes in `ui/src/api.ts` are hand-maintained. There is no codegen-stale guard for them, so the Rust struct and the TS interface can drift silently — e.g. `PrFeedbackItem` gained a `thread_id` field that had to be mirrored by hand in both places.

Scope of the evaluation:
- Inventory which response/request types in `types.rs` have hand-written mirrors in `ui/src/api.ts`.
- Assess cost/risk of adding `#[derive(ts_rs::TS)]` + `#[ts(export, export_to = "../ui/src/generated/")]` to them, including types that embed `serde_json::Value` (likely need `#[ts(type = "unknown")]`) and any nested types.
- Decide whether to extend the `./dev.py check` git-diff-exit-code guard to cover these generated files.
- Either migrate them, or document why the hand-mirror pattern stays and add a lighter-weight drift guard.

Pre-existing convention, not a regression — but the silent-drift risk is real and correct-by-construction principles favor codegen over hand-sync.
