Prod startup logs no build identity: "Phoenix IDE starting" carries exe path, pid, and mode, but not the git SHA the binary was built from. deployed.sha is overwritten by each deploy, so once a redeploy happens the previously-running binary's version is unrecoverable -- exactly what made the 2026-06-10 Explore ID-hint recurrence slow to diagnose (a pre-fix binary was still serving while main already contained the fix).

Embed the build git SHA (and dirty flag) at compile time -- e.g. via a build.rs that shells out to git rev-parse, or the vergen crate -- and include it in the startup tracing::info! line. Bonus: expose it on a health/version endpoint so a running server can be checked without log access.

Acceptance: startup log line includes the commit SHA the binary was built from; ./dev.py prod deploy output and deployed.sha agree with it.

## Implementation note

The SHA was already embedded at compile time (`build.rs` ->
`PHOENIX_GIT_SHA`) and exposed via `GET /api/version` and
`GET /api/deployment`; what was missing was the startup log line and a
dirty-tree marker. Delivered: `git_sha` + `version` fields on the
"Phoenix IDE starting" log line, and a `-dirty` suffix on the embedded SHA
when the working tree has uncommitted changes at build time.
