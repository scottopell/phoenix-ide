# ADR-087: Close tombstone recovery reinspects writers across all Linux tasks

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-WL-002b, REQ-WL-002c, REQ-WL-006, REQ-WL-007; final-tombstone recovery, Linux cwd inventory

## Context

A worktree can move from its captured path through quarantine into a recorded final tombstone before a crash. A writer scan at the old location does not prove the moved object is safe to remove on recovery. Persisted root/object bindings prove which object Phoenix owns, not whether an ambient process can still write to it.

Linux permits a task other than its thread-group leader to hold a different working directory. Process inventories can change or reuse identifiers while Phoenix reads procfs. Filtering by Phoenix ownership or effective UID, skipping vanished/unreadable tasks, or scanning only leader cwd can turn incomplete observations into an unsafe clean result. The private namespace remains the bounded reliability boundary chosen by ADR-042; this is not general adversarial deletion.

## Options considered

1. **Reuse pre-crash scans and inspect only leaders or same-user processes.** Cheaper and less likely to block, but cannot establish safe removal of the current object.
2. **Discover and terminate ambient writers.** Can force cleanup, but ambient identity is not Phoenix ownership and does not grant signal authority.
3. **Fresh all-process/all-task observation and fail-closed recovery.** Preserves uncertain resources without claiming universal process containment, at the cost of repair on unreadability or churn.

## Decision

Choose option 3. Recovered tombstone deletion freshly scans process cwd and open-descriptor writer evidence at the actual object. Validate the owner-only no-follow root and descriptor-relative object against their persisted device/inode and captured fingerprint, then revalidate after scanning before deletion. A writer or indeterminate inspection preserves the object and atomically records the exact attempt/scope residual and `NeedsRepair`; it grants no ambient signal authority.

On Linux, cwd inspection includes every numeric process in the visible namespace and every task, regardless of UID or Phoenix ownership. A clean result requires stable process and task sets plus incarnations verified with proc-directory device/inode and stat ID, start time, code range and stack start. Access denial, malformed/incomplete observations, disappearance, PID/task reuse, exec or inventory churn are indeterminate, not evidence of a clean scan.

Crash recovery distinguishes a recorded root before rename, a bound moved object, a moved unbound object, and exact completed absence. A root with no moved object may resume only from exactly one verified captured/quarantine source and after fresh writer inspection. An unbound moved object remains repair input. Completed absence requires same-attempt durable authority and positively missing captured/quarantine/object locations; inaccessible or replaced paths do not count. `NeedsRepair` still requires explicit retry under ADR-086.

## Consequences

- **Positive:** Non-leader and cross-UID cwd holders cannot be silently omitted from Linux retirement safety.
- **Positive:** Restart does not turn an old clean scan or a tombstone pathname into deletion permission.
- **Negative:** Procfs access restrictions or ordinary inventory churn can require explicit repair/retry even when no writer is eventually found.
- **Negative:** Crash points without a durable moved-object identity may leave safe leftovers requiring manual repair.
- **Neutral:** Visible process-namespace coverage and the private-directory trust boundary are explicit; malicious same-user/privileged mutation inside that namespace remains unsupported.

## References

- [Work lifecycle requirements](../work-lifecycle/requirements.md), REQ-WL-002b/002c and REQ-WL-006/007
- [Work lifecycle behavior](../work-lifecycle/work-lifecycle.allium)
- ADR-042: Close directory retirement trusts its private namespace
- ADR-080: Close stops bound fsmonitor daemons instead of reconfiguring Git
- ADR-086: Close retry adopts exact cleanup lineage without automatic repair dispatch
- `linux_namespace_cwd_scan`, `linux_cwd_process_incarnation`, `linux_cwd_task_inventory`, `resume_final_worktree_tombstone_with_writer_inspection`
