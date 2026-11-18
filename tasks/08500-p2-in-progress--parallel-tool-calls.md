# Run independent foreground ordinary tool calls concurrently

The user authorized a current provider audit, an adversarially reviewed bounded design, implementation, and a separate PR through green CI. The verified implementation base is `9b8b33a3398605db9dbde0cb2de69fbf355d362c` on `codex/concurrent-tool-batches`.

A native provider response containing only independent ordinary calls forms one foreground batch. The model chooses independence. Bash and patch may overlap under existing permissions and Work admission. No conflict detection, locks, dependency graph, external retry/replay, backgrounding, or new crash/exactly-once guarantee is included. Any lifecycle-owned special call keeps the whole response on the existing serial path. The initial per-conversation bound is four retained preparation/execution owners.

## Reviewed design

The core's existing current call and remaining calls remain the single ordered collection of unresolved calls. Completed sibling results enter the existing completed results immediately after their owning state commit, even when an earlier sibling is slow. Runtime owners retain only per-call selection/preparation/task/token/outcome/deadline capabilities. The final complete round is checkpointed once in original assistant call order before one model continuation. No second durable activity store or schedule is introduced.

Each selection captures exact runtime, original full assistant message and parsed call before read awaits. Layer0 proof precedes ordinary generation, channel, task, token, context, starts and metrics admission. Ordinary errors continue siblings; required preparation/persistence failure retains its existing fail-stop policy. The leading source call retains Caller preparation-error provenance; later source calls retain the required tool-command boundary even when admitted concurrently.

Cancel revokes read work, prevents refill and signals every admitted ordinary sibling immediately. Each owner retains its own one-shot receiver: already-typed ready completions survive a Cancel branch selected first; a raw tool return not yet converted into an outcome retains existing abort semantics. The aggregate drains all unresolved members before checkpoint and model/terminal advance. One absolute cancellation deadline retires remaining members; actual unknown local persistence authority closes the shared fence. A cancelling sub-agent reaches its existing Failed/NotifyParent destination only after every sibling settles or is retired.

Existing startup materialization preserves accepted completed results, synthesizes interruption for unresolved calls, and emits the full round in source order with metadata selected by identity. It never re-executes unknown-effect calls. This does not promise live runtime replacement or new recovery behavior. UI activity distinguishes actual starts, queued pending calls and accepted completions, scoped to the assistant round.

## Provider audit and design review

Anthropic preserves multiple ToolUse blocks without disabling their emission. Platform Responses and Chat Completions enable native multiple calls. Native Codex Responses Lite keeps its existing false parallel flag, matching current upstream `openai/codex` at `44de9d99ef5a9ea3a04c00e09bec8ebd67707868`; no unsupported wire toggle or tool wrapper is added. The executor accepts any valid native multi-call response. Provider audit is source-based, not a live-provider test.

The independent adversarial design review sealed v2 as workable after requiring aggregate child cancellation and source-ordered interruption materialization. Scratch audit/design artifacts are `/tmp/08500-provider-audit.md` and `/tmp/08500-concurrent-batch-design-v2.md` (SHA256 `44443ef3ab0d6f4961cbfe87df0893118fe12a7377322fe9bb41f6894c132420`). These are review evidence, not normative repository artifacts.

## Acceptance and qualification

- Deterministic held first call permits later siblings to start and finish; cap four and refill are exact.
- Same-name distinct IDs, out-of-order success/error, complete pairing and one final model continuation.
- Policy/scope/catalog cancellation, no pre-gate resources, no late launch or queued start after Cancel.
- Typed ready result versus Cancel race, duplicate/stale/reused IDs, sender loss and exact source ownership.
- Required sibling preparation failure and real SQLite settlement failure/unknown authority remain fail-stop.
- Per-call Bash wait deadlines cannot be renewed by sibling state transitions or cancel the represented process.
- Parent and child cancellation drain all ordinary members before final checkpoint/terminal notification.
- Mixed native/special paths preserve restrictions, Work admission, activation and fan-in authority.
- Interrupted partial rounds preserve accepted payloads/metadata and source order without replay.
- Provider route request/normalization/history tests and truthful web/iOS multi-call activity/reconnect tests.

Validation uses deterministic barriers, managed Python 3.13/dev.py helpers, Bash, original TMPDIR, workers four, CARGO_INCREMENTAL=0 and unique `/tmp/08500-*` logs. Focused/owning/static checks precede an immutable candidate, repeated independent full-range reviews and one explicit-base normal default check. The root coordinator owns remote PR/CI work. Implementation is present; this task remains in-progress pending final qualification and review.

## Evidence ledger

- Initial pure regressions: three intended failures for out-of-order reducer admission, child drain and restart source order; corrected core/DB owning selection later passed 224/224.
- Actual actor overlap/error/barrier started RED 0/2 and passed 2/2 after per-call ownership. Subsequent focused coverage includes cap/refill, ready-receipt cancellation, held policy/scope, required sibling read errors, exact source/reused IDs, independent wait cutoffs, child drain, special serial tails and lost outcome sender. Oracle/fixture authoring failures remain in the unique logs; they are not behavioral passes.
- Real registered Bash FIFO journey and six expanded provider fixtures passed. Provider coverage proves native call preservation and full paired history, not live-provider emission or a changed Lite wire contract.
- Real SQLite batch state-commit rejection proves no refill, accepted publication or model advance after failure. Existing native real-DB classification tests remain owning coverage; the new shared-fence priority probe explicitly latches the fence and is not an injected SQLite unknown-result test.
- Owning01 was 467/471; owning02 was 472/474. Named corrections preserve incoming deferred-intent cancellation, explicit unissued tail receipts and real batch observation. These earlier runs remain RED, not final qualification.
- Held-write causal REDs on both actual routes and the real final checkpoint exposed delayed cancellation. After bounded mailbox custody, timer RED was 1/4; corrected same-write resource deadline service passed the original four probes. Queued-steer fixture-only failures are recorded separately; the corrected journey exposed lost instruction delivery, then passed with deferred uncommitted drain and guarded dispatch.
- `/tmp/08500-held-resource-focused-02.log`: 5/5 PASS, exit 0 at `f4a765496`. Both routes signal running tokens within 100ms of enqueue under deterministic virtual time, retire uncooperative tasks at the original cutoff before write release, withhold semantic acknowledgement until classification and retain exactly one forced notice. Wait fallback retires only its task; the true final checkpoint admits no old-round request. Queued instructions are preserved and receive one legitimate request after cancellation. This is control timing evidence, not a measured SQLite latency guarantee.
- UI focused selection: 736/736 across 11 files after correcting immutable no-activity memoization; UI static lanes 4/4. iOS build-for-testing is environment-blocked (iOS 26.5 platform/destination absent), with no Swift compile or simulator pass claimed. No platform installation or unrelated configuration change was made.

Current owning suites, all-target Clippy/static gates, final clean immutable review candidate and normal explicit-base default are pending. No hosted, browser, deployed or final acceptance is claimed by this ledger.
