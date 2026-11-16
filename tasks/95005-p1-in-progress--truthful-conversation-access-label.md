# Make conversation mode access labels truthful

Correct Product and unknown/missing mode labels that implied Full access. Keep mode provenance distinct from effective persisted WorkScope authority; approval may leave Explore/Product mode unchanged. Reuse existing role data for Coordinator tools and preserve runtime permissions.

## Scope
- Explicit Product design/approval-scoped display; no false read-only or blanket Full access claims.
- Conservative missing/new mode display, supported Work/Branch/Direct/Approved Task/Work Child and Coordinator identities.
- Regress unapproved Product and rerendered mode/role transitions; do not infer authority from task or scope presence.
- Independent delta review, hosted UI/type/lint validation, normal merge and exact postmerge CI.
- No production deploy/restart, local native builds/installs, or permission expansion.

## Verification
Focused identity/StateBar suite: 132 tests passed after review corrections; changed-file ESLint, StateBar stylelint and git diff checks passed. Independent delta review found unsupported-host Bash wording and new badge modifier styling; both were corrected and a final independent review found no blocker. Hosted qualification and integration receipts will be recorded on the PR.
