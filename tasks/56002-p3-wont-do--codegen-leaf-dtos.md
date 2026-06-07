WONT-DO: relocate ts-rs-exported wire DTOs from phoenix_ide into phoenix-core to shrink the serial codegen pre-step in `./dev.py check`.

## Hypothesis
The codegen pre-step (~95s warm CI) is a GLOBAL serial pre-req (UI lanes wait for ui/src/generated). It is slow because the 5 ts-rs export sites live in phoenix_ide (dep-graph root, drags chromiumoxide/reqwest/axum/tokio). Move the DTOs to the featherweight phoenix-core (serde-only) and scope codegen to `cargo test -p phoenix-core` -> codegen ~95s -> ~5s.

## What was built (prototype, abandoned)
Moved SseWireEvent/ChainSseWireEvent/EnrichedMessage/ErrorPresentation/EnrichedConversation/SseBreadcrumb/UserFacingError/deployment+chain DTOs into phoenix-core; scoped codegen to phoenix-core. Verified clean: generated tree byte-identical, no #[ts(export)] left in phoenix-ide, gate green. Standalone codegen measured 95s -> 4.5s.

## Why WONT-DO: it does not improve CI wall time
The codegen head was NOT wasted serial time. The clippy-last reorder (PR #236) already made codegen double-serve as lane_rust`s test build (the 2.1s reused `cargo test compile` in warm CI). The true critical chain is:

    compile full workspace (~95s) -> rust test-run (~144s) -> clippy (~36s) = ~275s

That chain is unavoidable: the rust tests need the full first-party compile (deps cached, first-party never is), and compile->run->clippy are sequential within lane_rust. e2e (~131s) and the UI lanes are all SHORTER, so they ride under it. Relocating codegen just moves the 95s compile from "serial head" into "lane_rust test-compile" (losing the reuse) -- same chain length:

| | before (#236) | after (this prototype) |
| serial codegen head | 95s | 4.5s |
| lane_rust test-compile | 2s (reused) | ~90s (full, no reuse) |
| critical path | ~277s | ~275-279s |

Shrinking the head only helps if the UI lanes were the bottleneck waiting on it -- they are not.

## Added cost that sealed the decision
The orphan rule forced From<Message>/enrich_content/bash-display-merge into phoenix-core, whose charter is explicitly "serializable types with no business logic, the acyclic base." Net: no CI win + a layering-charter violation.

## What actually shortens the chain (follow-ups)
1. clippy into its own parallel lane (-36s; sccache from #236 makes the separate target dir safe). <- doing this next.
2. the ~144s rust test-run (bigger runner to lift the 3-thread cap on 4 cores, or split slow network-gated tests).

Prototype branch (local, not merged): codegen-leaf-dtos. This record is the durable artifact.
