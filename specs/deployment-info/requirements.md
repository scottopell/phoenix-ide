# Deployment Info

## Scope

A read-only "About this deployment" page reachable from the settings surface in
the conversation list. It answers one operator question: *what exactly is this
running instance, where does it keep its data, and how much of the machine is it
using right now?*

The page is primarily diagnostic. It changes no server state while reading build,
network, resource, disk, or log facts. The disk section may expose narrowly scoped
cleanup actions for backend-confirmed leftover Phoenix-managed worktrees. Every
value it shows is a fact the running process already knows or can measure: build
identity, runtime configuration (network binding, TLS), live resource usage,
on-disk locations with their sizes, and the path to the log file.

Logs are surfaced as a **path only** — the page never renders log contents.

## User Story

As the operator of a Phoenix instance (often myself, on my own box), I open a
session and want to confirm what's actually running and where its bytes live —
without SSHing in to read env vars, `du` the data directory, or `ps` the
process. I need Phoenix to tell me:

- Which version and exact build (git SHA) this is, and how long it's been up
- What address/port it's bound to and whether TLS is on, in which mode, with
  which cert/key
- How much memory and CPU the process is using right now, against the machine's
  totals
- Where on disk it stores its database, certs, extracted skills, credential
  files, and caches — and how big each of those is
- Where the log file is, so I can go read it myself

## Transparency Contract

The page must let me confidently answer:

1. Is this the build I think it is? (version, git SHA, uptime/start time)
2. How is it reachable, and is the connection encrypted? (bind address, port,
   TLS mode + cert/key paths)
3. Is it healthy on this machine right now? (process memory, CPU, system totals)
4. Where are its bytes, and how many? (on-disk locations + sizes)
5. Where do I go to read what it logged? (log file path)

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-DEPLOY-001: Reach "About this deployment" from settings

THE SYSTEM SHALL provide an "About this deployment" entry within the settings
surface reachable from the conversation list

WHEN the user opens that entry
THE SYSTEM SHALL render a dedicated read-only page (its own route) showing the
deployment facts described by REQ-DEPLOY-002 through REQ-DEPLOY-006

THE SYSTEM SHALL NOT expose controls on this page that mutate server state except
for the backend-revalidated leftover worktree cleanup action described by
REQ-DEPLOY-008

**Rationale:** The existing settings menu is a floating dropdown sized for a few
toggles. "About this deployment" is a dense read-only report, not a control, so
it gets its own page rather than crowding the dropdown. Keeping ordinary
inspection separate from the explicitly scoped cleanup action means the page is
safe to open while still giving leftover Phoenix-created disk usage an in-product
resolution path.

---

### REQ-DEPLOY-002: Report build identity and uptime

THE SYSTEM SHALL display:

- The application version (the crate version, e.g. `0.8.1`)
- The build identifier (the git short SHA compiled into the binary)
- Process uptime
- The wall-clock time the process started

WHEN the build identifier is unavailable at compile time
THE SYSTEM SHALL display the build identifier as `unknown` rather than omitting
the field

**Rationale:** "Which build is this?" is the first question when a deployment
misbehaves. Version alone is ambiguous across rebuilds of the same version; the
git SHA pins the exact source. Uptime plus start time together answer "did it
just restart?" without forcing the reader to do clock arithmetic. The `unknown`
sentinel is preserved from the build's own contract so a missing SHA reads as a
known-absent value, not a rendering bug.

---

### REQ-DEPLOY-003: Report network binding and TLS configuration

THE SYSTEM SHALL display the address and port the server is bound to

THE SYSTEM SHALL display whether the listener was provided via socket activation
versus bound directly

THE SYSTEM SHALL display the TLS configuration:

- Whether TLS is enabled
- WHEN TLS is enabled, the mode (`auto` self-signed vs. `manual` provided certs)
- WHEN TLS is enabled, the certificate path, key path, and CA certificate path
  (when present)
- WHEN TLS is enabled in auto mode, the host names the certificate is generated
  for

WHEN TLS is disabled
THE SYSTEM SHALL state plainly that the server is serving plain HTTP

**Rationale:** "How do I reach it and is it encrypted?" needs the bind target and
the TLS posture in one glance. The auto-vs-manual distinction matters because an
auto self-signed cert explains a browser trust warning that a manual cert would
not. Surfacing cert/key paths lets the operator find and inspect the actual
files. Socket activation changes how the process was launched (systemd handed it
the socket), which is worth knowing when the bind address looks surprising.

---

### REQ-DEPLOY-004: Report live managed-resource and host usage

THE SYSTEM SHALL provide a dedicated managed-resource endpoint distinct from the
general deployment snapshot and the disk-sizing endpoint.

THE SYSTEM SHALL expose live CPU, memory, load, and managed-process telemetry only through the dedicated managed-resource endpoint, not through the general deployment snapshot.

THE SYSTEM SHALL display host-wide resource usage including:

- Logical CPU count
- CPU busy/idle state derived from sampled CPU percentages
- CPU busy and idle percentages
- System CPU percentage when available
- Total, available, and used system memory
- Load averages over one, five, and fifteen minutes

THE SYSTEM SHALL display Phoenix-managed resource usage as a separate aggregate
including:

- Current total CPU utilization across attributed managed processes
- Current total memory across attributed managed processes
- Total managed process row count
- Deduplicated managed PID count

THE SYSTEM SHALL attribute managed usage by category, with explicit categories
for:

- API
- Bash
- Browser
- tmux/terminal
- MCP

THE SYSTEM SHALL treat API, Bash, and shell-mode terminal process groups as attributable categories when native process identity is available.

THE SYSTEM SHALL deduplicate shared PIDs before reporting the managed aggregate
so the same native process is not double-counted across categories.

THE SYSTEM SHALL keep Browser, tmux/terminal, and MCP as explicit categories EVEN WHEN some or all native process identity is unavailable, and SHALL report the capability limitation with a reason rather than silently omitting it.

THE SYSTEM SHALL provide per-process rows for every attributed managed PID,
including:

- Process name
- Category
- PID
- CPU percent
- Memory bytes
- Thread count
- CPU time seconds

WHEN a per-process metric cannot be sampled on the host platform or for that
process, THE SYSTEM SHALL report that field as unavailable (`null` on the wire)
rather than a misleading zero.

THE SYSTEM SHALL log per-process metric sampling failures at debug level or above.

THE SYSTEM SHALL measure resource values on both macOS and Linux.

**Rationale:** The operator question is broader than "what is the Phoenix server process using?" The deployment page answers "what resources are Phoenix and the processes it manages using, and how busy is the host around them?" Host memory, idle/busy state, and load make the machine context self-interpreting; category attribution distinguishes assigned managed pressure from explicit attribution capability gaps.
PID deduplication is load-bearing because one native process must not inflate the
managed total simply by appearing in more than one attribution path. Nullable
per-process fields preserve the difference between "zero" and "not observable on
this host or for this process."

---

### REQ-DEPLOY-004A: Poll live resource data while the page is visible

THE SYSTEM SHALL refresh the managed-resource endpoint approximately once per
second while the deployment page is visible.

THE SYSTEM SHALL suspend the initial managed-resource fetch and all periodic polling while the document is hidden.

WHEN the page becomes visible after being hidden
THE SYSTEM SHALL promptly request a fresh managed-resource sample.

THE SYSTEM SHALL avoid overlapping managed-resource requests.

THE SYSTEM SHALL ignore managed-resource completions that arrive after the observing effect has unmounted or been superseded.

**Rationale:** Resource monitoring is useful only when it stays fresh, but
continuous background polling for a hidden tab wastes work and distorts the
operator's mental model of "what I'm actively watching." Roughly one-second
polling keeps the page live without claiming hard real-time semantics, and the
no-overlap rule prevents stacked requests from turning temporary slowness into a
self-inflicted load spike.

---

### REQ-DEPLOY-004B: Maintain bounded rolling history and rollups

THE SYSTEM SHALL maintain a bounded client-side history of recent good
managed-resource samples covering at most five minutes.

THE SYSTEM SHALL derive rolling summaries from that bounded history,
including current, average, and peak values for:

- Managed CPU utilization
- Managed memory usage

THE SYSTEM SHALL present the bounded history as recent-over-time data rather than
as an unbounded historical archive.

**Rationale:** Operators need short-horizon trend context — "is this spike new or
sustained?" — without turning the deployment page into a durable monitoring
system. Five minutes is enough to distinguish a blip from a pattern while keeping
storage bounded and semantics local to the open page.

---

### REQ-DEPLOY-004C: Preserve last-good semantics across refresh failures

WHEN a managed-resource refresh fails after at least one good sample has been
captured
THE SYSTEM SHALL retain the last good sample and bounded history instead of
clearing them.

WHEN the page is showing retained data after a failed refresh
THE SYSTEM SHALL mark that resource display as stale and surface the refresh
error.

WHEN no good sample has been captured yet and a refresh fails
THE SYSTEM SHALL report the failure without fabricating resource data.

**Rationale:** A transient backend or transport failure should not erase the last
known-good picture the operator was using. Marking that picture stale preserves
honesty: the page remains useful while clearly distinguishing retained data from
a newly confirmed sample.

---

### REQ-DEPLOY-004D: Keep deployment facts and live resource monitoring separate

THE SYSTEM SHALL keep the managed-resource endpoint and refresh cycle separate
from the general deployment snapshot and from disk sizing.

THE SYSTEM SHALL allow live resource refresh without requiring build, network,
log, or disk facts to reload.

**Rationale:** Build identity, TLS posture, and on-disk layout are relatively
static compared with live resource data. Splitting the live monitor into its own
endpoint avoids coupling a fast-refresh surface to slower or unrelated data
fetches.

---

### REQ-DEPLOY-005: Report on-disk locations and their sizes

THE SYSTEM SHALL load on-disk locations through a disk-specific API surface so the
page can render build, network, resource, and log facts without waiting for
recursive disk sizing.

THE SYSTEM SHALL display the on-disk locations the deployment uses, each with its
semantic category, absolute path, and, where determinable, its current size:

- The SQLite database file
- The data directory root (the parent that holds the database and other state)
- The TLS directory (when TLS is configured)
- The extracted built-in skills directory
- Credential / auth files written by the deployment
- The attachment store: the directory holding file-based attachments when that
  storage mode is active, or an explicit indication that attachments are stored
  inline in the database
- Phoenix-managed git worktrees created under project `.phoenix/worktrees/`
- Temporary and cache directories used for ephemeral work (e.g. browser caches
  and per-scope browser profiles)

For individually-named files and for directories known to be small, THE SYSTEM
SHALL report a measured size (recursing into small directories).

For directories that may be large or expensive to walk (e.g. browser binary
caches, per-scope browser profiles), THE SYSTEM SHALL report the path WITHOUT a
recursively-computed size, and SHALL indicate that the size was not measured
rather than reporting it as zero.

WHEN a listed path does not exist on disk
THE SYSTEM SHALL show it as absent rather than omitting the row or reporting a
size of zero

THE SYSTEM SHALL give disk sizing its own loading, error, refresh, and sampled-at
state independent of the general deployment facts.

THE SYSTEM SHALL make the Phoenix-managed worktree aggregate expandable into
per-worktree rows sorted by measured size descending, with unmeasured or absent
rows ordered predictably after measured rows.

THE SYSTEM SHALL render managed-worktree row semantics from typed backend
disposition, not by interpreting labels or path strings.

FOR a live/non-terminal managed worktree, THE SYSTEM SHALL show an action to open
the owning conversation and SHALL NOT offer direct deletion from the deployment
page.

FOR a leftover managed worktree, THE SYSTEM SHALL show cleanup only when the
backend disposition says cleanup is allowed.

**Rationale:** "Where are my bytes?" is the disk-pressure question. The data
directory is what an operator backs up, copies, or deletes; the caches are what
they clear first when reclaiming space. Sizing the small, owned artifacts is
cheap and useful; recursively walking a multi-gigabyte browser cache on every
page load is neither, so those are shown as paths with an explicit "not
measured" marker. Distinguishing absent from zero-size matters: an absent TLS
directory means TLS was never configured, which is different from a configured
directory that happens to be empty. Managed worktrees are listed because they are
Phoenix-created project checkouts and can dominate disk usage; terminal or
archived conversations do not imply live worktree ownership, but any DB-known
managed worktree path that still exists on disk is still Phoenix-created disk
usage. The attachment store is listed even while attachments live inline in the
database, so the row is a stable home for the file-based attachment directory
once that storage mode is active — the reader always finds attachment storage in
the same place, whether it resolves to a directory or to "inline in the
database."

---

### REQ-DEPLOY-006: Surface the log sinks, never the contents

THE SYSTEM SHALL report every log sink the running logger is configured to write
to. Logging fans out to independent sinks, each individually enabled:

- WHEN logs are written to standard output THE SYSTEM SHALL indicate the stdout
  sink is active (captured by the supervising process)
- WHEN logs are written to a process-owned file THE SYSTEM SHALL display that
  file's absolute path
- WHEN a sink is not active THE SYSTEM SHALL indicate its absence rather than
  implying output that is not produced

THE SYSTEM SHALL NOT render log file contents on the page

**Rationale:** The page's job is orientation, not log viewing. Showing the path
is a one-step handoff to tools built for tailing and searching; rendering
contents would turn a lightweight diagnostic page into an unbounded data view and
risk leaking sensitive log lines into the browser. The sinks are independent
because a deployment may want stdout (for a supervisor's journal), a file (for an
operator to tail), or both at once — nothing structurally couples them. The
honesty caveat is load-bearing: a path is shown only for a file the process
*itself* writes, never a launcher redirection the process cannot guarantee. The
reported sinks are derived from the same configuration that builds the logger, so
the report and the wiring share one source of truth and cannot disagree.

---

### REQ-DEPLOY-007: Freshness of sampled values

THE SYSTEM SHALL identify deployment snapshots, managed-resource samples, and
disk samples as point-in-time measurements.

WHEN the user requests a general refresh
THE SYSTEM SHALL re-sample the general deployment snapshot and SHALL trigger a
fresh managed-resource sample rather than serving cached values.

WHEN the user requests a disk-only refresh
THE SYSTEM SHALL re-sample disk values without requiring the general deployment
snapshot to reload.

**Rationale:** Memory, CPU, and disk sizes drift continuously. A value with no
notion of when it was taken invites the reader to trust a stale number. Sampled
attribution and timestamps keep each surface honest about whether it is a fresh
measurement, retained last-good data, or a separate disk sample taken on its own
cadence.

---

### REQ-DEPLOY-007a: Shared demand-driven resource observations

WHEN one or more deployment, Work Scope, or process-inspector surfaces request live process metrics
THE SYSTEM SHALL derive their overlapping metrics from one timestamped observation generation over a deduplicated set of Phoenix-managed process identities.

WHEN concurrent consumers request metrics within the observation freshness lease
THE SYSTEM SHALL coalesce them onto the same generation rather than repeat native process discovery and CPU measurement.

WHILE no resource consumer requests observations
THE SYSTEM SHALL perform no recurring process sampling.

THE SYSTEM SHALL preserve unavailable metrics as unavailable rather than zero, and SHALL protect process attribution against PID reuse by validating native process identity across the sampling interval.

**Rationale:** CPU measurement requires an interval and proportional-memory reads are operating-system work. One demand-driven generation amortizes that work and gives overlapping surfaces consistent values without creating a permanent monitor when nobody is looking.

---

### REQ-DEPLOY-008: Safely clean up leftover managed worktrees

THE SYSTEM SHALL expose a mutation endpoint for cleaning up a Phoenix-managed
worktree only after backend revalidation.

WHEN cleanup is requested
THE SYSTEM SHALL re-check that:

- the path is one of the database-known managed worktree paths
- the path matches the strict Phoenix worktree shape `{repo}/.phoenix/worktrees/{id}`
- no live, non-terminal, non-archived conversation owns that worktree
- the operation matches persisted mode semantics: Work-mode cleanup may remove the
  Phoenix-created branch after removing the worktree; Branch-mode cleanup removes
  only the worktree and preserves the user branch

WHEN the directory is already absent
THE SYSTEM SHALL treat cleanup as a successful idempotent no-op.

THE SYSTEM SHALL reject cleanup for unknown paths, malformed/non-Phoenix paths,
and worktrees still owned by a live conversation.

**Rationale:** A leftover managed worktree is Phoenix-created disk usage, but the
filesystem is still a user-owned environment. Cleanup therefore belongs behind a
server-side proof obligation, not a UI convention. The backend already has the
conversation state, archived flag, and persisted mode semantics needed to avoid
deleting live work or user branches.
