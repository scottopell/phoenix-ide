# Observe a conversation fixture

Observation serves persisted conversation reads from a private, standalone SQLite
fixture through an explicitly selected Phoenix binary. It is not a live development
server, fixture collector, sanitizer, replay executor, or production deployment.
See [requirements](../specs/conversation_observation/requirements.md) and the
[current evidence and gap matrix](../specs/conversation_observation/executive.md).

## Launch without building or loading development configuration

Supply an **already-built, trusted binary that implements `--observe-fixture`**:

```sh
./dev.py observe \
  --fixture /absolute/private-fixtures/example/fixture.db \
  --target EXPLICIT_CONVERSATION_REFERENCE \
  --binary /absolute/candidate/phoenix_ide
```

The paths are placeholders, not known host locations. `--binary` is required; a
relative binary path is resolved against your invocation directory. There is no
build, binary discovery, migration, seeding, fixture creation, data acquisition,
provider bootstrap, Vite startup, browser opening, or automatic fallback to normal
server mode. Older binaries must fail on the unsupported invocation; never rerun
without the observation flags to work around that failure.

### Portable debug artifacts

The artifact producer must build the exact UI before building the existing binary
with its opt-in embedding feature:

```sh
corepack pnpm --dir ui run build
CARGO_BUILD_JOBS=1 cargo build -p phoenix_ide --bin phoenix_ide --features portable-ui
CARGO_BUILD_JOBS=1 cargo test -p phoenix_ide --features portable-ui \
  --test observation_startup portable_ui_serves_exact_built_entry_assets_from_private_cwd \
  -- --test-threads=1 --nocapture
```

These producer commands are not implicit launcher actions or permission to compile
on a constrained consumer. `portable-ui` forwards `rust-embed/debug-embed`; default
development and release behavior remain unchanged. Default debug binaries may
require the build machine's filesystem and are not portable merely because a UI
build passed. The feature-gated startup guard uses a private cwd without `ui/dist`,
checks root HTML and its actual content-hashed module entries, verifies JavaScript
MIME/nonempty bytes, and prints SHA-256 values matching the built files. Retain
that manifest with source/tree, selected feature and binary SHA. On the receiving
machine, verify root and entry-asset HTTP responses before browser qualification.

`--port 0` (the default) lets the OS choose a free loopback port. The server prints
`Observation listening on http://127.0.0.1:PORT` after admission and binding; use
that actual URL, not a guessed port. A nonzero `--port` requests that exact port;
an occupied port is an error, not authority to stop its owner. This command does
not inspect, stop, reconfigure, or replace other owned services.
No dev PID, port, proxy, log, or production service files are managed.

The command runs in the foreground, forwards SIGINT/SIGTERM to its child, and
returns the child's exit status (signal exits use `128 + signal`). Stop it with
Ctrl-C. Scratch HOME, XDG config/data/cache/state, working directory and temporary
directory are private to that invocation and removed after the child exits.
The child receives only these directory variables plus fixed C locale; no inherited
provider keys, database settings, proxies, tracing exporters, dynamic-loader flags,
or PATH. Dev tracing and pretty-output bootstrap are bypassed even with global
`--pretty`. The script's ordinary uv interpreter/dependency launcher still applies;
this is not an OS sandbox or a promise of zero interpreter setup. The chosen binary
is trusted executable code and the operator remains responsible for its provenance.

## Fixture admission and scope

The **server is authoritative**, not Python or the manifest. The launcher checks
argument shape and executable availability; it does not open SQLite, chmod a
fixture, copy it, checkpoint it, fix it, remove sidecars, or judge schema fitness.
Supply an independently prepared and reviewed snapshot, never a live database.
Server admission requires an absolute regular, chmod-read-only file, rejects
production-like canonical locations and hard links, and rejects `-wal`, `-shm`
and `-journal` sidecars at both supplied and canonical paths. Do not make a live
database read-only to satisfy admission. The read-only DB path does not migrate;
unsupported schema/read requirements and invalid targets must fail without repair.
Path checks and read-only mode are not proof that data was sanitized or authorized.

The observation application admits one resolved ordinary or coordinator
ProductConversation and supported persisted history/route/list reads. It does not
construct live runtime services. Unsupported/live capabilities are unavailable;
do not interpret their errors as permission to enable a provider or full server.
Loopback access is unauthenticated; the server rejects forwarded requests. Keep the
port private: do not publish a tunnel or proxy. A local browser or process can read
admitted data. The matching UI discovers capabilities before mounting ownership
and selects persisted-only history rather than live streams, activation, provider
catalogs, host-resource polling or telemetry reporting. It discovers the admitted
target through `GET /api/observation/target`, reads ordinary history through
`GET /api/product-conversations/{reference}?boundary_representation=original_messages`,
and reads Global history through `GET /api/global/coordinator/history/{reference}`.
Those history responses include `latest_opening`, `boundaries` and explicit
`event_presentation` fields (including null) on messages. The live member endpoint
`GET /api/conversations/{member}/observation` remains unavailable (503); it is not
required by this persisted-only UI. Capability discovery fails closed. Actual nested-widget requests, UI usability and endpoint denial still need
separate binary/browser qualification; mocked tests alone do not prove this boundary.

## Portable fixture manifest, format version 1

Keep `manifest.json` beside a private `fixture.db`. This is a **documented handoff
format**, not a launcher input or an implemented JSON Schema validator. `observe`
does not verify it. A producer and reviewer must check it before sharing a fixture;
missing identity must remain `null` where allowed, never inferred from another
artifact. Do not include absolute paths, usernames, hostnames, credentials,
production conversation text, or live resource locators in a portable manifest.
Use relative bundle filenames and opaque, sanitized fixture-local references.
Never check private fixtures, manifests, captures, or screenshots into git.

All fields below are required; additional fields require a new format version.
SHA-256 means 64 lowercase hex characters over the exact artifact bytes.

| Field | Type and meaning |
|---|---|
| `format_version` | Integer `1` |
| `kind` | String `conversation-observation-fixture` |
| `fixture` | Object `{file: "fixture.db", sha256: SHA256, bytes: positive integer}` |
| `schema` | Object `{user_version: nonnegative integer, migration_ledger_sha256: SHA256, sqlite_schema_sha256: SHA256}` |
| `source` | Object `{kind: "synthetic" or "sanitized_snapshot", revision: full source commit or null, authorization_ref: opaque nonsecret review reference}`; no source DB path |
| `binary` | Object `{sha256: SHA256, source_commit: full commit or null, version: string or null}` for the explicitly selected executable |
| `ui` | Object `{sha256: SHA256 or null, source_commit: full commit or null}`; UI hash is independent of backend identity |
| `sanitizer` | Object `{name: string, version: string, rules_sha256: SHA256}`; synthetic generators also identify their version/rules |
| `privacy` | Object `{version: 1, classification: "private-sanitized", reviewed: true, review_ref: opaque nonsecret reference}` |
| `targets` | Nonempty array of unique fixture-local conversation reference strings |
| `limits` | Object with positive integer ceilings `fixture_bytes`, `conversations`, `messages`, `text_bytes_per_message`, `artifact_bytes`, `processes`, `wall_seconds` |
| `counts` | Object with nonnegative integers `conversations`, `messages`, `max_text_bytes_per_message`, `artifact_bytes` |

Canonical digest inputs: `migration_ledger_sha256` hashes UTF-8 JSON of an array of
`[version, name]` tuples from `_migrations` ordered by migration version;
`sqlite_schema_sha256` hashes UTF-8 JSON of `[type, name, tbl_name, sql]` tuples
from `sqlite_schema`, ordered by type/name/table/SQL, preserving SQL nulls and
including internal entries. Both use compact JSON separators `,` and `:`, no
trailing newline, and literal UTF-8 characters (not ASCII escapes). These are
identity records, not authorization for schema migration or repair. For separately
available UI assets, `ui.sha256` hashes compact UTF-8 JSON `[relative_path, SHA256]`
tuples ordered by relative POSIX path. For an embedded UI without an independently
verifiable asset inventory use `null`; do not reuse the binary hash as a UI hash.

A bounded handoff profile can set ceilings to 268435456 fixture bytes, 32
conversations, 10000 messages, 32768 UTF-8 bytes per message, 2097152 artifact
bytes, 1 server process and 600 wall seconds. These are **operator/producer
limits**, not enforced launcher quotas or guarantees on SQLite query memory.
Choose and record stricter limits where appropriate; validate counts and file size
against them, and externally bound execution/capture. Fixture creation and
sanitization require their own explicit authority and implementation; this command
never acquires data by default. Hashes prove identity, not privacy. Review bodies,
titles, tool arguments/results, paths, URLs, attachments, structured metadata and
relational references; token replacement alone is not sufficient sanitization.

## Next-worker handoff

Provide only a private bundle locator through an authorized channel, the manifest,
exact binary identity, fixture-local target, and the qualification still owed. The
next worker chooses its own absolute paths, verifies hashes/privacy/count limits,
and launches with the command above. Record actual server admission, actual URL,
observed route and UI behavior separately from source tests. Do not copy another
machine's HOME, port assignment, provider environment or personal file paths.
No fixture or observation evidence has been produced merely by adding this command.

## Browser measurement protocol

Use the existing `browser_profile.run_scenario` harness; do not create another
runner or treat this command as a benchmark. First verify the exact binary/UI
identities, actual ordinary and Global response shapes and denied live endpoints.
Put a synchronous boolean readiness wait first: the harness excludes setup through
that first wait from its page-anchored measurement window. An async expression is
not a readiness boolean; do not rely on Promise coercion. Set reset, CPU throttle,
warmups and GC explicitly, and preserve raw samples and methodology warnings.

For list-click journeys, a reload after a prior run may begin on the conversation
route. Return through an actual Home/list control before the first readiness wait,
or independently reset to the list outside a one-sample invocation. Record which
setup was used; do not silently substitute reset-disabled SPA results. Verify
meaningful transcript content, navigation presence, a bounded stable anchor window
and actual scrolling rather than merely a URL or spinner disappearing.

Separate reload/warm-cache, fresh browser context and explicitly disabled HTTP
cache experiments. None proves cold SQLite/OS caches. Calibrate same-artifact A/A
noise before interpreting balanced A/B pairs, and retain failures and outliers.
Nonsignificance or insufficient power does not establish neutrality. There is no
numeric regression allowance implied by this runbook. Capture CPU profiles and
traces in separate attribution runs; long-task sums are not total JS CPU, nested
trace tasks are not additive CPU, and double-animation-frame marks are presentation
estimates rather than compositor-paint measurements. An observation composition
with telemetry unavailable cannot measure collector/export overhead.
