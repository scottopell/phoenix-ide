# Exact-main geometry source integration audit

## Source and authority

The integration retains the normal two-parent history of local predecessor
`d65e064481f13762923d3dc6ae5511a1661e942f` and exact published main
`9060b11ff5ec9bb9c5b7b4a080be1aa619f095c6` (#952). It imports main's complete
geometry implementation, not scripts copied into an older app tree. The only
path modified by both histories was `.github/workflows/ci.yml`; its additions
merge independently. The geometry job, including its browser gate and main
failure aggregation, is unchanged from that main source.

The separate stock app admission still explicitly checks out
`f16afb227d9336a863c0b0345960ea9a6d62b7c7` (tree
`369305dcd94862d4ee12e7af9fd463d5da14d808`). Inheriting main's app changes in this
harness does not change that admitted app, module hashes, natural sample rate,
watchdogs, thresholds, or fixture route. Stage 1/context-close and Stage 2/actual
reporter-pagehide remain separately opt-in; the negative offline guard on the
stock runtime job is preserved.

## Discoverability — DONE (source only)

| Surface | Committed location |
|---|---|
| Workflow and checkout authority | `.github/workflows/ci.yml`, `check-finalization-geometry`: one checkout of `${{ github.event.pull_request.head.sha || github.sha }}`; captures run from `ui`; `source.txt` records actual checkout HEAD |
| Prose geometry capture | `ui/scripts/capture-assistant-finalization.mjs` |
| Table capture | `ui/scripts/capture-streaming-table.mjs` |
| Reader/pinned transcript capture | `ui/scripts/capture-assistant-finalization-transcript.mjs` |
| Sub-agent task-details capture | `ui/scripts/capture-subagent-task-details.mjs` |
| Prose/table/transcript fixtures | `ui/src/fixtures/assistantFinalization/`: `acl-reply.md`, `renderFixture.{tsx,css}`, `tableFixture.{tsx,css}`, `transcriptFixture.{tsx,css}` |
| Ladle entrypoints | `ui/src/stories/`: `assistant-finalization.stories.tsx`, `streaming-table.stories.tsx`, `assistant-finalization-transcript.stories.tsx`, `subagent-task-details.stories.tsx` |
| Component regressions | `AssistantMessageChrome.test.tsx`, `StreamingMessage.tables.test.tsx`, `StreamingMessage.disclosures.test.tsx`, `MessageList.test.tsx`, `SubAgentTaskDetails.test.tsx`, `SubAgentViewerPanel.test.tsx` under `ui/src/components/` |
| Source-only preflight | `tests/e2e/history_stock_prepare.test.py`: `geometry_source_preflight` and seven geometry tests; existing unconditional offline UI step runs this suite |
| Normative contract and status | `specs/conversation-ui/requirements.md`, REQ-CONV-019; `specs/conversation-ui/executive.md`, Assistant Finalization Geometry Qualification |
| Diagnostic tools and results | [stock packet](../../tests/e2e/history-stock-prepare.md), [primary diagnostics](stock-f16-primary-diagnostic.md), [offline proof](stock-f16-lifecycle-offline-proof.md), [context-close result](results/context-close-39146439582/README.md), [actual reporter result](results/reporter-pagehide-39148822485/README.md) |

## Validation boundaries

**DONE (docs/preflight only):** the refined preflight reads the workflow and
resources from the selected committed Git head, not ambient filesystem bytes.
It parses the actual geometry job's exact four entrypoints, resolves the relative
story/fixture imports and raw fixture URL, checks the checkout-head expression,
`source.txt` command and source-hash input blobs, and checks geometry membership
in **both** main failure aggregators. Synthetic temporary-repository negatives
reject a missing entrypoint before a mock installer can be called, even when an
ambient copy exists; missing fixture/story, changed checkout/source/hash commands,
a missing capture command, and either omitted aggregator dependency also fail.
Dirty ambient bytes do not change the selected-head source/hash observation.
The geometry job invokes the selected-HEAD CLI immediately after checkout and
before pnpm, setup, or dependency installation. The CLI and regression tests
neither import nor execute a capture entrypoint. This is an early source-only
admission gate, not new source authority or geometry qualification.

**NOT DONE:** new browser/script capture execution, Ladle/server launch, builds,
dependency installation, SQL, full `dev.py check`, hosted CI, push, label changes,
production deployment, new natural samples, or performance/physical-device
acceptance. **Exact-successor hosted geometry qualification is NOT DONE.** The
inherited component regressions are discoverable source, not new passing test
claims. Parent/UI owner confirmed that the four capture scripts remain an atomic
geometry gate with REQ-CONV-014/019, REQ-MLRU-010 and virtual-anchor ownership;
none is transplanted, skipped, or relaxed here.

The historical stock failure remains **inconclusive**. The retained synthetic
context-close schedules did not reproduce a response after successful context
close. The actual reporter probe was functionally red at the departing-document
204 observation boundary; it does not establish a production collector/export
violation or recover the historical lost primary/cleanup-only assertion. The
existing results and conditional patch proposal remain evidence, not authority
to change the pinned app or relax delivery/cleanup gates.

## Publication — BLOCKED; execution-policy gate unresolved

The supplied `offline-regression-only` label description is: “Run offline
regression checks; explicitly skip benchmark runtime capture”. Its implemented
stock guard is benchmark-specific. Main's geometry job is an automatic browser
job whenever `needs.plan_check.outputs.ui == 'true'`; the label does **not**
suppress it. A PR update may therefore execute geometry even with that label.
The parent's no-new-browser policy gate is unresolved. No workflow guard, label
policy, runtime code, or normative requirement changes here. Source-preflight
success does not satisfy the exact-successor hosted geometry gate or authorize
publication. This handoff is **NOT READY** for push/CI; no further policy proposal
or implicit geometry exemption is adopted.

### Completed early source gate and discoverability audit

The geometry job now invokes
`python3 -B tests/e2e/history_stock_prepare.test.py --geometry-source-preflight HEAD`
immediately after its selected-head checkout and before pnpm/setup/install. This
source-only CLI verifies the committed checkout identity, all four capture
entrypoints and their story/fixture dependencies, source SHA inputs, and both
failure aggregators. It neither installs nor executes a capture. Negative tests
reject missing committed resources even when ambient files exist.

The bounded documentation audit is complete: preparation/opt-in navigation,
historical authorization status, actual Stage 1/2 result links, conditional-model
limitations, telemetry executive checkpoint and full collector source location
are updated. Exact result/provenance JSON and normative requirements are unchanged.

Local source regressions: 26 preparation/preflight, 14 publication and 8 artifact
controls passed. Exact-successor hosted geometry remains NOT DONE. Publication
remains BLOCKED by the no-new-browser execution constraint: the inherited geometry
job still launches a browser, and its policy has not been changed or waived.
