Close retirement false-positives "open descriptors can still modify the confirmed worktree" on any host with core.fsmonitor=true globally configured, because every git subprocess Phoenix spawns leaves a detached fsmonitor--daemon holding an open descriptor on the worktree, which quarantine_has_open_descriptors cannot distinguish from a genuine external writer.

Fixed by moving the core.fsmonitor=false override from three ad hoc close_retirement.rs call sites into phoenix_core::git::command()'s shared noninteractive config, closing the gap for every git invocation Phoenix makes (not just the three that had been patched). Added a direct regression test (command_disables_hostile_fsmonitor_configuration) proving the property.

PR: fix-fsmonitor-disable-for-git-subprocesses branch, commit ee64a43a5.
