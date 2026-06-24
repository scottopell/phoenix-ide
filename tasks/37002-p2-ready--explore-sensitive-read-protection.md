# Explore sensitive read protection

## Summary
Explore mode is intentionally read-only/network-blocked, not a confidentiality boundary. Sandboxed Explore bash currently follows the same broad local-read risk model as `read_file`/`search`, while constraining writes, network, and ambient credential environment variables.

If Phoenix should protect sensitive readable paths (for example `.ssh`, `.aws`, Codex/Phoenix data dirs, or procfs process environments), design it as a separate feature with an explicit threat model instead of overloading the sandboxed bash PR.

## Acceptance Criteria
- [ ] Define which local reads must be protected and from whom.
- [ ] Specify cross-platform semantics for Linux Landlock, macOS Seatbelt, and unsupported hosts.
- [ ] Decide whether protection applies only to bash or also to `read_file`, `search`, `keyword_search`, and image/file viewers.
- [ ] Update specs before implementation.
