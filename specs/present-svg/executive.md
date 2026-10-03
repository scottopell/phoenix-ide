# Durable inline SVG presentation — Executive Summary

## Requirements Summary

Agents publish code-generated static SVG files as durable inline charts. Users can expand, inspect source, and download without accessing server paths. Explore publication is excluded because its staging lifetime is not reliably cross-call.

## Technical Summary

Strict XML/SVG validation precedes an atomic conversation-owned database snapshot. Existing tool-result text carries a compact typed reference; web UI reads it into an image-only card. Authenticated routes match conversation and artifact IDs. See ADR-058.

## Status Summary

| Requirement | Status | Evidence |
| --- | --- | --- |
| REQ-SVG-001 | Implemented | Typed invocation and storage-enforced presentation metadata, Direct/Work registration and Explore/coordinator exclusion tests; real agent publication |
| REQ-SVG-002 | Implemented | XML-aware allowlist, bounded reference graph, property applicability and parent-child models, adversarial unit cases and actual Matplotlib fixture |
| REQ-SVG-003 | Implemented | Atomic database snapshot, deterministic ambiguous-commit classification, invocation replay, commit-before-checkpoint restart recovery in executing/cancelling states, rollback, reopen and transcript deletion tests; live source deletion and server restart |
| REQ-SVG-004 | Implemented | Router authentication/owner/header tests; live image, source, attachment and direct-navigation checks |
| REQ-SVG-005 | Implemented | Component tests, 12 browser fixture journeys and actual conversation controls/reload |
| REQ-SVG-006 | Implemented | Typed input roundtrip, ordinary persisted tool-result reference, runtime/SSE E2E and native generic-result inspection |
| REQ-SVG-007 | Implemented | Bounded category-specific errors, tool description and generation guide |

## Verification

### Automated qualification

- `cargo test -p phoenix-svg`: validator tests cover supported library output, hostile XML/CSS/URLs, compatible reference targets, exact selector grammar, per-element geometry, reference cycles/expansion and geometry/complexity limits.
- `cargo test -p phoenix-tools present_svg`: publication tests cover file boundaries, cancellation, replay, reused provider IDs in separate assistant messages, and bounded persistence errors. The database publication API requires private-field `ValidatedSvg` and `SvgPresentationMetadata`; compile-fail doctests reject raw bytes and raw metadata. Deterministic commit fault tests distinguish committed snapshots, confirmed absence, and unclassified local authority.
- `cargo test -p phoenix-db svg_artifact`: storage tests cover immutable snapshots, reopen after staging deletion, actual workscope retirement after worktree-directory removal, separate invocations, owner isolation, cascade deletion, rejected writes and transaction rollback.
- `cargo test -p phoenix_ide svg_artifact`: actual router authentication, ownership, accepted bytes and response headers for all three retrieval routes.
- `uv run tests/e2e/run.py --scenario present_svg`: mock-provider turn dispatch generates a file with bash, publishes through the runtime, replaces/deletes staging, checks retrieval/ownership, reloads persisted references through HTTP and SSE init, and publishes a separate revision when a later assistant round reuses the provider tool ID.
- `./dev.py check`: Rust/UI/codegen/spec/task/integration checks. The first broad run caught a test-helper temp-path lint (corrected) and a tmux watchdog cleanup timeout. The exact watchdog test passed alone, and the final full Rust lane passed along with codegen, TypeScript and UI lint (seven checks, 493.6s). Final spec/task/fmt checks also passed (five checks).
- The shared dialog Escape regression and full Vitest lane pass: dismissal consumes the event before global conversation navigation sees it.

### Browser and live-agent evidence

`LADLE_PORT=61128 ./dev.py qa svg-artifacts` exercises 14 combinations covering light/dark, full/compact, desktop/mobile, very tall/wide charts, long titles and anonymous shared conversations. The shared page receives token-scoped artifacts while protected owner URLs return 401. The capture script verifies controls and escaped source, and emits screenshots and `hostile-browser-evidence.json` under ignored `ui/qa-artifacts/svg-artifacts/`.

Hostile browser fixtures deliberately bypass ingestion to independently test image-context and serving-policy defenses. Script/event, external image/CSS and navigation payloads produce no external requests or application mutation in preview/expansion; direct navigation downloads and retains a blank document. Production ingestion separately rejects those payloads.

A real GPT-5.5 Direct conversation generated and published both a labelled disk-usage SVG and an actual Matplotlib chart through `present_svg`. Both staging files were removed after success. The accepted disk chart bytes were identical before and after `./dev.py restart`. An actual browser loaded the persisted cards, expanded them, dismissed dialogs with Escape without leaving the conversation, displayed escaped source, downloaded the snapshot, and reloaded successfully with zero external requests. Local screenshots and `verification.json` are under ignored `ui/qa-artifacts/svg-artifacts-live/`.

The disk chart keeps free space separate from measured directory bars and shows exact GiB labels. Native iOS compatibility was checked by inspecting its generic tool-result decoding/rendering path; no device run or native SVG renderer is claimed. Workscope retention uses the existing retained transcript owner, with no dependency on staging/worktree paths. No production deployment was performed.

### Review qualification

The validator and metadata suite covers incompatible presentation properties, property-specific color/baseline values, font-family grammar, stylesheet applicability with preserved universal/inherited styling, resource/text content models, clip reuse targets, unsupported `tspan` transforms, and Unicode metadata boundaries. The actual Matplotlib fixture remains accepted byte-for-byte.

PR #793 includes validation-backed bytes and metadata at storage, assistant-scoped publication/result identity, committed-reference restart recovery, and exact classification after ambiguous commits. Runtime guard tests verify established outcomes, fail-stop without semantic results when authority is unclassified, unexpected owner loss, closed admission, and the explicit coordinated-shutdown exception. Child transcript tests cover full/peek rendering and child-owned access beneath a parent share context. Shared preview/source/download are tested without a password and after token revocation. The expanded runtime E2E and all 14 browser journeys pass.

The local broad Rust run exposed an existing tmux probe test that reads a PID file after a 500 ms deadline; under host load the child can be killed before writing that file. The standalone test passed, but the owning module reproduced the race. Task 45020 records the missing causal test witness; the SVG implementation does not alter production tmux behavior. PR #793 carries the current hosted checks and Codex review state.
