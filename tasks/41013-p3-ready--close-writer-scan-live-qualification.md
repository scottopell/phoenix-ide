Close retirement external-writer scans lost their live-syscall qualification when the host-dependent tests were removed from the default module (close_retirement.rs). The admission decision is now proven deterministically over an injectable observation seam, and the scans are covered by fake-/proc _in variants, but these live round trips are no longer exercised in-suite:

- linux: real mmap -> /proc/<pid>/maps, real /proc/<pid>/cwd, and PR_SET_DUMPABLE same-user credential attribution.
- macos: proc_pidinfo(PROC_PIDVNODEPATHINFO) cwd path and the writable-mapping (proc_pidregionpathinfo) path have NO deterministic test at all.
- end-to-end quarantine_and_remove_exact_worktree Residual-on-open-descriptor path is unqualified live.

Decide whether to re-add these as an explicit opt-in qualification lane (separate from the default gate) or accept the gap. See specs/work-lifecycle/executive.md "Writer-evidence scan coverage after live-test removal".
