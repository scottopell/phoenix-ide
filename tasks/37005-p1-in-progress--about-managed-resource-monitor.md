# Make `/about` a live Phoenix resource monitor

## Problem

The Resources section currently takes one request-time sample of only the `phoenix-ide` API process and shows five text rows. This materially understates Phoenix's footprint: Phoenix also owns WorkScope-keyed bash process groups, browser sessions, and tmux/terminal resources. The page provides no trend, peak, or moving average, so short spikes and sustained pressure are indistinguishable.

Phoenix already has most of the ownership and sampling foundation needed:

- `RuntimeManager` owns the WorkScope-keyed bash, browser, tmux, and terminal registries.
- `BashHandleRegistry::snapshot_live_pgids` can enumerate live bash groups across all scopes.
- The process inspector already samples cross-platform group CPU, proportional/shared-aware memory, and process count.
- `recharts` is already a UI dependency.

The implementation should reuse those authorities rather than infer ownership from command names or duplicate registry state.

## Outcome

Turn the `/about` Resources section into an Activity Monitor–style, live diagnostic view of the footprint Phoenix is responsible for:

1. Show host context: used/available/total memory, host CPU utilization, logical CPU count, and load average where supported.
2. Show a clearly defined **Phoenix managed total** alongside the API process. The total covers the API process plus live WorkScope-owned process resources and must avoid double-counting overlapping PIDs/process groups.
3. Break the managed total down by resource owner/type, at minimum API, bash, browser, and tmux/terminal, with process count, CPU %, and memory. Unsupported or not-yet-attributable categories are explicit rather than silently omitted.
4. Refresh automatically while `/about` is visible and stop immediately when it unmounts or is hidden.
5. Show bounded recent history for managed CPU and memory, including current, rolling average, and peak. A five-minute client-side window at roughly one sample per second is sufficient; history need not survive page reload or server restart.
6. Keep manual refresh and honest freshness/error states. A failed poll must retain the last good sample, mark it stale, and avoid fabricating zeroes.

The UI should communicate equivalent operational information to Activity Monitor, not clone its chrome. Use compact summary cards, a category table, and small CPU/memory time-series charts with units and definitions visible.

## Design constraints

- Use typed Rust wire types exported through `ts_rs`; regenerate checked-in TypeScript via `./dev.py codegen`.
- Keep static deployment facts and expensive disk sizing separate from the live resource endpoint. Add a resource-specific endpoint rather than polling all of `GET /api/deployment` or disk data every second.
- Build one request-time snapshot from authoritative registries. Do not persist a second copy of WorkScope ownership.
- Represent attribution structurally (typed category/owner records and explicit availability), not through UI interpretation of labels.
- Deduplicate by native PID before totals are computed. Define CPU semantics explicitly: process/category CPU may exceed 100% on multicore hosts; host CPU is normalized to 0–100%.
- Prefer proportional/shared-aware memory where the existing process-group sampler supports it. Name any fallback metric precisely; never label RSS as footprint/PSS.
- Sampling capability gaps must be `null`/typed unavailable and logged at debug or above.
- Do not make recursive process discovery the sole ownership authority. If browser or tmux ownership does not currently expose enough native identity for attribution, extend its owning registry/session type with the minimal PID/PGID identity captured at spawn, then project it read-only.
- Count only live process resources in current totals. Tombstoned bash handles remain inventory/history records but consume no current CPU or memory.
- Keep the recurring work bounded: one sampler pass per poll, no background sampling when no client is watching, bounded client history, and no database writes.

## Work plan

### 1. Specify the monitoring contract

Update `specs/deployment-info/requirements.md` and `executive.md` to replace the snapshot-only resource contract with live, managed-footprint observability. Remove the existing timeless requirement/rationale that declares streaming/history out of scope. Define metric meanings, attribution coverage, deduplication, cadence, bounded history, stale behavior, and platform capability gaps. Do not add a new legacy `design.md`; avoid extending stale v1 design material.

### 2. Extract a reusable process sampler

Refactor the process-inspector's platform implementation into a reusable sampler that can accept a deduplicated set of PIDs/process groups and return CPU, proportional memory, and process count without changing inspector behavior. Add tests for overlap/deduplication, exited-during-sample races, null capability results, and multicore CPU totals.

### 3. Enumerate globally managed process ownership

Add read-only snapshot accessors to the WorkScope registries as needed. Assemble typed resource targets across all scopes, preserving owner/type for breakdown while deduplicating PIDs for the grand total. Cover:

- the Phoenix API PID;
- live bash handle process groups;
- live browser session process trees/groups;
- tmux/terminal server or PTY process trees/groups.

Audit other Phoenix-owned long-lived child managers (notably stdio MCP clients) and either include them under a typed category or return an explicit unsupported/not-attributable category with a follow-up requirement; do not silently claim the managed total is exhaustive when it is not.

### 4. Add a focused live resource API

Add an authenticated resource endpoint returning timestamped host metrics, API metrics, deduplicated managed totals, and typed category/owner breakdowns. Keep `/api/deployment` compatible for build/network/log consumers or deliberately migrate its resource field with parity tests. Ensure sampling does not hold async registry locks across the CPU sampling interval.

### 5. Build the live `/about` monitor

Replace the text-only resource rows with:

- host pressure and Phoenix managed-total summary cards;
- an expandable category table showing CPU, memory, and process count;
- managed CPU and memory charts over the bounded rolling window;
- current, rolling average, and peak values;
- sampled-at, polling/stale, unavailable, and retry states.

Use a roughly one-second poll only while the page is visible. Use `AbortController`/cleanup to prevent overlapping requests and updates after unmount. Keep history client-local and reset it explicitly on page reload.

### 6. Verify

Add backend unit/integration coverage for attribution and aggregation, route/auth behavior, platform sampling degradation, and non-overlap. Add React tests with fake timers for polling start/stop, hidden-page suspension, rolling-window eviction, moving average/peak calculation, stale samples, and unavailable metrics. Run `./dev.py codegen`, focused Rust/UI tests, and `./dev.py check`.

Manually compare a busy Phoenix session against macOS Activity Monitor (and Linux `ps`/`top` where available) using a known CPU/memory-consuming background handle and active browser. Document expected differences caused by proportional memory and multicore CPU definitions rather than forcing numerically misleading parity.

## Acceptance criteria

- With no managed children, the managed total approximately matches the API row and clearly says what it includes.
- Starting a busy background bash handle increases the bash row and managed CPU/process totals without double-counting the API.
- Starting and stopping a browser or terminal/tmux resource adds/removes its typed category contribution.
- CPU and memory charts update automatically, retain only the configured rolling window, and expose current/average/peak.
- Leaving or hiding `/about` stops polling; returning resumes with an honest gap rather than invented samples.
- A sampler failure leaves the last good values visible but marked stale/unavailable.
- Metrics are named and normalized consistently enough to reconcile with Activity Monitor/top, with platform-specific differences explained.
- Disk sizing is not invoked by the live polling path.
- Full codegen and project checks pass.
