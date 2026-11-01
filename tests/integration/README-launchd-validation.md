# PreparedSource114 launchd validation artifact

## Authority and scope

[The provenance manifest](../../.github/artifacts/prepared-source114-aarch64.json)
is the single operational source for the exact Actions artifact, producer job,
candidate source, bundle filenames, recorded hashes, qualification receipts, expected
validation output, expiry, and reproduction policy. Do not copy identifiers into a
second manifest or substitute a similarly named artifact.

The recorded object is an **ephemeral Actions artifact**, not a release or durable
retained copy. Its disposition is deterministic reproduction after expiry. A new run
produces new evidence: it must receive a new identity and must never be described as
artifact 12122737139 or as byte-identical without an independently established hash.
The old PreparedSource114 candidate is not the current production repair candidate.

No download, rerun, build, signing, publication, installation, service control,
migration, or production operation is authorized by this document. Commands below
are bounded procedures for a separately authorized owner.

The repository workflow retrieves these bytes only on an explicitly authorized
`workflow_dispatch` with `validate_prepared_source114=true`; ordinary pull-request and
push runs skip both retrieval and the joined controller-to-validator integration. The
manual integration consumes a real persisted `ReadyForDisruption`/`VerifiedClean`
controller result before it invokes the fixed validation harness, then removes the
retrieved bytes with an `always()` cleanup step. Checkout credentials are not persisted
into repository-controlled code.

## Bounded metadata verification

Use an authenticated identity with repository Actions-read access and a reviewed
checkout containing the manifest. Metadata verification does not download bytes.
The recorded workflow run failed in an unrelated Intel matrix job; admission is bound
to the exact successful ARM job rather than to the overall run conclusion.

```sh
set -eu
umask 077
manifest=.github/artifacts/prepared-source114-aarch64.json
jq -e '.schema_version == 1 and
       .record_kind == "prepared_source114_actions_artifact_provenance" and
       .artifact.retention.kind == "ephemeral_actions_artifact"' "$manifest" >/dev/null
repo=$(jq -er '.repository.full_name' "$manifest")
artifact_id=$(jq -er '.artifact.id' "$manifest")
run_id=$(jq -er '.producer.workflow_run_id' "$manifest")
job_id=$(jq -er '.producer.qualified_job.id' "$manifest")
: "${GH_TOKEN:?An authorized Actions-read token is required}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
api="https://api.github.com/repos/$repo"
api_json() {
  curl --fail --silent --show-error --connect-timeout 10 --max-time 60 \
    --max-redirs 0 --max-filesize 1048576 \
    -H "Authorization: Bearer $GH_TOKEN" \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' "$1"
}

gh repo view --json nameWithOwner,databaseId > "$work/current-repository.json"
jq -e --slurpfile m "$manifest" '
  .nameWithOwner == $m[0].repository.full_name and
  (.databaseId | tonumber) == $m[0].repository.id
' "$work/current-repository.json" >/dev/null

api_json "$api/actions/artifacts/$artifact_id" > "$work/artifact.json"
jq -e --slurpfile m "$manifest" '
  $m[0] as $p |
  .id == $p.artifact.id and .name == $p.artifact.name and
  .size_in_bytes == $p.artifact.size_in_bytes and
  .digest == $p.artifact.api_digest and
  .created_at == $p.artifact.created_at and
  .expires_at == $p.artifact.expires_at and .expired == false and
  (.expires_at | fromdateiso8601) > now and
  .workflow_run.id == $p.producer.workflow_run_id and
  .workflow_run.head_sha == $p.candidate.source_commit and
  .workflow_run.repository_id == $p.repository.id
' "$work/artifact.json" >/dev/null

api_json "$api/actions/runs/$run_id" > "$work/run.json"
jq -e --slurpfile m "$manifest" '
  $m[0] as $p |
  .id == $p.producer.workflow_run_id and
  .repository.id == $p.repository.id and
  .repository.full_name == $p.repository.full_name and
  .head_sha == $p.producer.workflow_commit and
  .path == $p.producer.workflow_file and
  .event == $p.producer.workflow_run_event and
  .head_branch == $p.producer.workflow_run_branch and
  .run_attempt == $p.producer.workflow_run_attempt and
  .status == "completed" and .conclusion == $p.producer.workflow_run_conclusion
' "$work/run.json" >/dev/null

api_json "$api/actions/jobs/$job_id" > "$work/job.json"
jq -e --slurpfile m "$manifest" '
  $m[0] as $p |
  $p.producer.workflow_run_conclusion == "failure" and
  $p.producer.qualified_job.conclusion == "success" and
  .id == $p.producer.qualified_job.id and
  .run_id == $p.producer.workflow_run_id and
  .run_attempt == $p.producer.workflow_run_attempt and
  .head_sha == $p.candidate.source_commit and
  .name == $p.producer.qualified_job.name and
  .conclusion == "success"
' "$work/job.json" >/dev/null

gh api --allow-escape-sequences \
  "repos/$repo/actions/jobs/$job_id/logs" > "$work/job.log"
grep -F -- "$(jq -er '.producer.operation_log_marker' "$manifest")" \
  "$work/job.log" >/dev/null
workflow_commit=$(jq -er '.producer.workflow_commit' "$manifest")
workflow_file=$(jq -er '.producer.workflow_file' "$manifest")
git show "$workflow_commit:$workflow_file" > "$work/producer-workflow.yml"
printf '%s  %s\n' \
  "$(jq -er '.producer.workflow_file_sha256' "$manifest")" \
  "$work/producer-workflow.yml" | shasum -a 256 -c -
jq -e '.producer.operation == "prepare-main" and
       .producer.inputs == {"operation":"prepare-main"}' "$manifest" >/dev/null

# One bounded page for the exact run; stop rather than paginate or search by name.
api_json "$api/actions/runs/$run_id/artifacts?per_page=100&page=1" > "$work/list.json"
jq -e --slurpfile a "$work/artifact.json" '
  .total_count <= 100 and
  ([.artifacts[] | select(.id == $a[0].id)] | length) == 1 and
  ([.artifacts[] | select(.id == $a[0].id)][0] |
    .name == $a[0].name and .digest == $a[0].digest and
    .size_in_bytes == $a[0].size_in_bytes and
    .expires_at == $a[0].expires_at and .expired == false)
' "$work/list.json" >/dev/null
```

## Retrieval, only after separate owner authorization

Download only the exact artifact ID. The cap is a transfer bound, not authenticity.
Do not add `--location-trusted`; authorization must not be forwarded to the signed
cross-host redirect.

```sh
archive_cap=$(jq -er '.artifact.size_in_bytes * 2' "$manifest")
curl --fail --silent --show-error --location --max-redirs 3 \
  --proto '=https' --proto-redir '=https' \
  --connect-timeout 10 --max-time 180 --max-filesize "$archive_cap" \
  -H "Authorization: Bearer $GH_TOKEN" \
  -H 'Accept: application/vnd.github+json' \
  -H 'X-GitHub-Api-Version: 2022-11-28' \
  "$api/actions/artifacts/$artifact_id/zip" -o "$work/prepared.zip"
printf '%s  %s\n' \
  "$(jq -er '.artifact.api_digest | sub("^sha256:"; "")' "$manifest")" \
  "$work/prepared.zip" | shasum -a 256 -c -
jq -r '.bundle.files[].name' "$manifest" > "$work/expected-files.txt"
```

Before extraction, inspect archive entry names and types. Reject absolute/traversing
paths, symlinks, duplicates, unexpected files, or sibling trees; require exactly the
manifest's five files. Extract only into a fresh owner-private directory, preserve the
archive and receipts unchanged, and compare every file with its non-null manifest
SHA-256. A null per-file digest means the old artifact has not yet yielded that value
to an authorized qualification log; it is never permission to skip real admission.

## Metadata is not byte admission

API identity, archive digest, filenames, and size do **not** authenticate executable
bytes or authorize deployment. Real admission still requires receipt/schema and full
candidate-commit binding, payload digest checks, Developer ID signature, hardened
runtime, notarization, stapling, Gatekeeper, and exact embedded commit/version checks
before candidate execution.

The candidate migration registry is materialized from the immutable candidate commit
and checked against `candidate.candidate_registry_sha256`, never from controller
working-tree bytes. The retained helper is materialized from the clean full controller
head being qualified; that head must differ from `candidate.source_commit`.

REQ-SDC-023 and `launchd_validation_harness.py` define the validation-only boundary.
Success is diagnostic only: no production claim, service control, backup, migration,
publication, activation, cleanup, recovery, signal, or guardian replacement authority.

## Joined nonproduction orchestration

The explicitly ignored macOS test
`installed_ready_controller_validates_prepared_source114_handoff_without_effects`
reuses the successful natural-root-exit controller journey. It admits the fixed Python
validation entrypoint only after a fresh, affine `LoadedAttempt` and the existing
`ReadyWorkflowSelection::Complete` prove durable `ReadyForDisruption` plus
`VerifiedClean`, with no pending native acknowledgement or retained guardian result.
The test passes no serialized readiness record, manifest field, executor command, or
mutation adapter across the boundary.

The read-only test capability accepts no caller-selected command: it resolves the
Python 3.12 interpreter installed by `actions/setup-python` beneath the hosted runner's
canonical tool cache, uses isolated Python startup, and invokes only
`tests/integration/launchd_validation_harness.py` with
`PHOENIX_PREPARED_ARTIFACT_ROOT`. The harness continues to call
`prepare_disposable_launchd_validation`, materialize the existing exact handoff, and
reach the shared `validate_pre_disruption_handoff` policy. It intercepts both helper and
`dev.py` mutation entrypoints in their real function globals, rejects any `launchctl`
subprocess, and rejects every SQLite path outside its generated owner-private root.
Manifest and staged-plist paths, test labels, databases, and loopback ports are checked
as disposable rather than production values. The controller SQLite main/WAL bytes and
loaded ready coordinates must remain identical across validation. The child receives a
parent-owned temporary directory, runs in its own process group, and has a 15-minute
bound; timeout kills the group, reaps the child within a second bound, and lets Rust
fixture and temporary-root teardown proceed.

A successful joined receipt contains the controller attempt/revision and the existing
harness line with ledger 113, all seven installer effects at zero, and explicit zero
production label/path/database observations. Fixture-owned launchd jobs and temporary
roots remain test lifecycle resources; they are not installer service-control or cleanup
effects. This proves orchestration continuity into read-only validation only. It grants
no installer adoption or production mutation authority.

## Deterministic reproduction after expiry

Reproduction pins the exact historical producer workflow and candidate source; it does
not preserve the old artifact identity or promise byte equality for signed/notarized
outputs.

1. Obtain explicit owner authorization for protected signing infrastructure and a new
   evidence identity. No authorization is implied here.
2. While GitHub still permits rerunning the historical run, the preferred pinned workflow
   operation is a rerun of `producer.workflow_run_id`. This preserves its workflow commit, source
   commit, event inputs, and matrix definition:

   ```sh
   gh run rerun "$run_id" --repo "$repo"
   ```

3. Require the resulting run attempt to retain the manifest's workflow commit,
   `prepare-main` input, exact ARM job, candidate source, signing/notary/stapler/spctl,
   helper, receipt, headless-acceptance, and upload gates. Record every new artifact
   under a new manifest; never overwrite this one.
4. If the historical run is no longer rerunnable, this record defines no exact runnable
   fallback. Pinned regeneration and requalification require a separately authorized workflow
   and new-evidence design. Dispatching current main, moving main, substituting the incident
   repair operation, or loosening its main-only gate is not equivalent and is not authorized.
5. Repeat real byte admission and hosted validation. A regenerated artifact is new
   evidence even if every recorded source/input field matches.

## Byte-identical retention

No owner-controlled byte retention is authorized. If a later requirement truly needs
the same bytes beyond the recorded expiry, the owner must explicitly authorize one
bounded download, independently verify the API digest and all recorded file hashes,
store immutable owner-controlled bytes plus retrieval evidence, and distinguish that
copy from a release, republished artifact, or regenerated artifact. The default retrieval
procedure deletes its private work directory on exit; retaining it requires that separate
owner authorization.
