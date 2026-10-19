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
| Source-only regression | `tests/e2e/history_stock_prepare.test.py`: `test_geometry_capture_resources_exist_in_committed_head`, `test_geometry_workflow_uses_matching_head_source_not_copied_scripts`; existing unconditional offline UI step runs this suite |
| Normative contract and status | `specs/conversation-ui/requirements.md`, REQ-CONV-019; `specs/conversation-ui/executive.md`, Assistant Finalization Geometry Qualification |
| Diagnostic tools and results | [stock packet](../../tests/e2e/history-stock-prepare.md), [primary diagnostics](stock-f16-primary-diagnostic.md), [offline proof](stock-f16-lifecycle-offline-proof.md), [context-close result](results/context-close-39146439582/README.md), [actual reporter result](results/reporter-pagehide-39148822485/README.md) |

## Validation boundaries

The two added regressions inspect Git's committed HEAD tree and workflow text;
they neither import nor execute any capture entrypoint. Before the merge commit,
both reject the predecessor HEAD, which lacks these resources and the geometry
job. Final integration validation is restricted to source regressions, cached
Node syntax checks, YAML parsing, and `bash -n` of workflow shell blocks.

**NOT DONE:** new browser/script capture execution, Ladle/server launch, builds,
dependency installation, SQL, full `dev.py check`, hosted CI, push, label changes,
production deployment, new natural samples, or performance/physical-device
acceptance. The inherited component regressions are discoverable source, not new
passing test claims. Parent retains UI-owner coordination with `58cff20d`.

The historical stock failure remains **inconclusive**. The retained synthetic
context-close schedules did not reproduce a response after successful context
close. The actual reporter probe was functionally red at the departing-document
204 observation boundary; it does not establish a production collector/export
violation or recover the historical lost primary/cleanup-only assertion. The
existing results and conditional patch proposal remain evidence, not authority
to change the pinned app or relax delivery/cleanup gates.

## Exact policy proposal — NOT IMPLEMENTED

The inspected `offline-regression-only` label description is: “Run offline
regression checks; explicitly skip benchmark runtime capture”. Its implemented
stock guard is benchmark-specific. Main's geometry job is an automatic browser
job whenever `needs.plan_check.outputs.ui == 'true'`; the label does **not**
suppress it. A future PR update may therefore execute geometry even with that
label. No push or hosted execution is authorized by this integration.

If a future source-only PR update is desired, obtain parent/UI-owner approval
for this exact narrow extension before publishing; do not silently reinterpret
the label as a global browser ban:

```yaml
# Proposed replacement only for check-finalization-geometry.if:
if: ${{ needs.plan_check.outputs.ui == 'true' && !(github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository && github.head_ref == 'prepare/exact-f16-stock-admission' && contains(github.event.pull_request.labels.*.name, 'offline-regression-only')) }}
```

The accompanying proposed label description is: “Run offline regression checks;
skip benchmark runtime capture and, on prepare/exact-f16-stock-admission only,
geometry browser capture; functional probes require separate explicit opt-in”.
A separately reviewed policy change must update the source regression's exact
geometry predicate and the offline-guard inventory assertion together. It must
leave Stage 1/2 opt-ins, stock negative guard, ordinary UI checks, other branches,
main pushes, and main failure aggregation intact. This is only a geometry-job
proposal, not proof that every other inherited CI job is runtime-free. Until
execution scope is resolved, retain the no-push/no-CI boundary.
