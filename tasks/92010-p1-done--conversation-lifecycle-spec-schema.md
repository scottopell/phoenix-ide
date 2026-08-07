# Gate 1 — Normative authority and schema contract

Child of task 92009. Update affected timeless requirements and Allium, add the ownership ADR, and establish normalized root lifecycle, typed provenance, durable Close phases/evidence, and deterministic legacy mapping with migration tests. Re-ground against landed durable direct-turn authority. Exit only when specs validate and every legacy mapping is covered.


## First deliverable review-clean summary

- Child spec package reviewed clean through final preflight QA task `92024`.
- Review inputs covered the full authoring/evidence chain: `92017` through `92023` plus `92025` through `92030`, `specs/AUTHORING.md`, ADR-025 / ADR README, and the affected requirements / Allium / executive artifacts.
- Validation gates passed:
  - affected Allium checked individually and in one combined run with zero `severity: "error"` diagnostics
  - `./dev.py check --lanes spec-shape,spec-anchors,allium`
  - `./dev.py tasks validate`
  - `git diff --check`
- Semantic authority checks passed for this deliverable:
  - singular product lifecycle remains Open/History
  - `continued_in_conv_id` remains sole continuation/latest topology authority
  - `WorkScope` remains resource owner, not lifecycle owner
  - approved-task / follow-up provenance and retrieval tombstone semantics remain typed and singular
  - Coordinator remains outside ordinary conversation lifecycle / WorkScope semantics
  - no targeted lifecycle artifact still depends on the removed lifecycle `design.md` files as current authority
- Residual non-blockers carried forward as implementation or broader-repo debt, not spec-package failures:
  - duplicate `REQ-WAKE-012` in `specs/wake-contracts/requirements.md` outside this deliverable scope
  - broader repo `design.md` references in unrelated spec families/tasks
  - previously documented implementation drift around Continue-here mode upgrade / fresh-continuation persistence
- Deliverable status: review-clean first spec package complete; implementation gates remain open under parent task 92009.
