# Deployment Info

## Scope

A read-only "About this deployment" page reachable from the settings surface in
the conversation list. It answers one operator question: *what exactly is this
running instance, where does it keep its data, and how much of the machine is it
using right now?*

The page is diagnostic, not interactive. It changes no server state. Every value
it shows is a fact the running process already knows or can cheaply measure:
build identity, runtime configuration (network binding, TLS), live resource
usage, on-disk locations with their sizes, and the path to the log file.

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

THE SYSTEM SHALL NOT expose any control on this page that mutates server state —
it is observational only

**Rationale:** The existing settings menu is a floating dropdown sized for a few
toggles. "About this deployment" is a dense read-only report, not a control, so
it gets its own page rather than crowding the dropdown. Keeping it strictly
read-only means it is always safe to open — there is no footgun in inspecting a
deployment.

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

### REQ-DEPLOY-004: Report live process and system resource usage

THE SYSTEM SHALL display the current process resource usage:

- Resident memory (RSS)
- CPU utilization

THE SYSTEM SHALL display the machine totals for context:

- Total and available system memory
- Logical CPU count

THE SYSTEM SHALL measure these on both macOS and Linux

WHEN a resource value cannot be sampled on the host platform
THE SYSTEM SHALL indicate the value is unavailable rather than reporting a
misleading zero

**Rationale:** A raw "RSS = 800 MB" is not actionable without the machine's total
— 800 MB is fine on a 64 GB box and alarming on a 1 GB box. Pairing process
figures with system totals makes the number self-interpreting. Both macOS and
Linux are first-class (the same binary is developed and run on both), so the
measurement cannot be Linux-only `/proc` scraping. The explicit unavailable
state preserves the correctness principle that silent zero is indistinguishable
from a bug.

---

### REQ-DEPLOY-005: Report on-disk locations and their sizes

THE SYSTEM SHALL display the on-disk locations the deployment uses, each with its
absolute path and, where determinable, its current size:

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
the same place, whether it currently resolves to a directory or to "inline in the
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

THE SYSTEM SHALL reflect that resource usage and on-disk sizes are point-in-time
samples taken when the page data was requested

WHEN the user requests a refresh
THE SYSTEM SHALL re-sample the live values rather than serving a cached snapshot

**Rationale:** Memory, CPU, and disk sizes drift continuously. A value with no
notion of when it was taken invites the reader to trust a stale number. Making
the sample explicitly point-in-time, with a way to re-sample, keeps the page
honest about what it is: a snapshot, not a live stream. (A live-streaming gauge
is deliberately out of scope — the operator question is "what is it now," not
"chart it over time.")
