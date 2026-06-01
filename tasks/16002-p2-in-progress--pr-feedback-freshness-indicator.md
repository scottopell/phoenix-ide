# Implement PR feedback freshness indicator

## Goal

Add a lightweight, stable “new comments” / “updated” indicator next to the Work Actions `Address CI & comments` remediation action, based on whether PR feedback changed since Phoenix last captured agent-facing PR remediation context.

The feature should stay deliberately simple:

- Do not move this signal into the StateBar; StateBar remains branch health / CI status.
- Treat successful `pr-auto-fix-context` capture as the semantic baseline for “what Phoenix has provided to the agent.”
- Avoid fetching all GitHub review surfaces on every routine PR status poll.
- Degrade gently when GitHub feedback surfaces are unavailable.
- Keep the remediation action available; freshness is advisory, not lifecycle authority.

## Plan

### Step 1 — Apply spec updates

Update `specs/projects/requirements.md` with new requirements:

- `REQ-PROJ-030: PR Feedback Freshness Indicator`
  - Show an advisory “new comments”, count, or coarse “updated” marker near the `Address CI & comments` Work Action.
  - Do not use PR feedback freshness as the StateBar branch-health signal.
  - Do not block cleanup, abandon, or ordinary conversation use based on freshness.

- `REQ-PROJ-031: Agent-Facing PR Context Baseline`
  - A successful PR remediation context capture is the baseline.
  - Store compact baseline data for work scope + PR number: capture timestamp, PR updated timestamp when available, and feedback identities/fingerprints when available.
  - “New” means current feedback contains identities absent from the latest successful baseline.
  - With no baseline, do not show “new comments.”

- `REQ-PROJ-032: Bounded PR Feedback Refresh`
  - Routine PR status refresh stays lightweight.
  - Full feedback surfaces are fetched only when gated by evidence such as PR `updated_at` being newer than the baseline, or by explicit remediation capture.
  - Failures degrade to no count / coarse advisory only and are logged.

Update `specs/projects/design.md` to document the split between:

1. StateBar branch health (`pr-status`), and
2. Work Action remediation freshness (`Address CI & comments`).

Update `specs/projects/executive.md` to list the new requirements as planned/in-progress/complete as appropriate after implementation.

If appropriate, update `specs/projects/projects.allium` only for non-UI semantics: successful remediation context capture records a baseline, and freshness is relative to the latest baseline. Do not model CSS/button placement in Allium.

### Step 2 — Edit code

Backend:

- Add persistence for a compact PR context baseline keyed by work scope + PR number.
- On successful `create_pr_auto_fix_context`, record/replace the baseline from the captured artifact.
- Add a lightweight freshness result to `PrStatusResponse` or a closely related API shape, preserving routine PR status as the main poll path.
- Use PR `updated_at` as the cheap gate before fetching full feedback for freshness classification.
- Fetch full feedback only when needed to classify freshness, and keep failures non-fatal.
- Represent feedback identity structurally, using provider ID, URL, or stable fingerprint fallback.

Frontend:

- Extend API types for PR feedback freshness.
- Keep StateBar unchanged for this feature except any type plumbing required.
- Update `WorkActions.tsx` so `PrRemediationActions` shows a small advisory marker next to `Address CI & comments`, e.g. `new comments`, `3 new`, or `updated`.
- Clear/update the marker after a successful remediation context capture by relying on the next status refresh or by triggering a local refresh if that is cleaner.
- Add/adjust tests for Work Actions behavior.

### Step 3 — Validate with automated checks

Run the relevant targeted checks first, then the full repo check:

- Rust tests covering PR monitoring / persistence behavior.
- UI tests covering Work Actions display states.
- Codegen if Rust API types change.
- `./dev.py check` before committing.

### Step 4 — Validate with browser tools

Use Phoenix’s dev workflow and browser automation to exercise the feature in the UI:

- Start/restart the app via `./dev.py` as needed.
- Navigate to a Work or Branch conversation with an associated open PR.
- Confirm StateBar still shows only branch health / CI status.
- Confirm Work Actions shows `Address CI & comments` normally when no baseline exists.
- Trigger PR context capture and confirm the action sends the artifact-backed message.
- Simulate or use a PR with newer feedback and confirm the Work Action shows a gentle freshness marker.
- Confirm the marker is cleared or updated after capturing new context.
- Check browser console for errors.

### Step 5 — Open PR and complete Codex review loop

After implementation and validation:

- Commit the completed work in logical commits.
- Push the branch.
- Open a PR.
- Comment `@codex pls review`.
- Wait for Codex review results.
- Address findings and repeat up to 3 review rounds, or stop once Codex reports no findings.
- Leave the PR in a clean state with tests passing and the executive spec status updated.

## Acceptance Criteria

- Specs define the feature’s semantics, placement, and rate-limit discipline.
- Successful PR remediation capture creates the source-of-truth baseline for freshness.
- Routine PR status polling does not fetch all PR feedback surfaces every time.
- Work Actions displays a compact advisory marker for new/updated PR feedback.
- StateBar remains focused on PR/CI branch health and does not show the new-feedback marker.
- Feature degrades gracefully when GitHub CLI/auth/API surfaces are unavailable.
- Automated tests and browser validation pass.
- PR is opened and Codex review is requested/addressed according to the review loop.
