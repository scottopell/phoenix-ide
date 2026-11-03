# ADR-219: Plugin removal revalidates entry identity and retains native paths

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-AP-006, REQ-AP-009, REQ-AP-010

## Context

The shared plugin root permits independent harness changes. A name/path selected
before MCP teardown can identify another directory after the asynchronous wait.
A retained path can also contain OS bytes that have no Unicode representation.
Using a lossy display string for that location destroys deletion authority.

## Decision

Capture the entry's filesystem identity before registry publication and teardown.
Revalidate device/inode identity on supported Unix hosts and reject a changed
entry immediately before deletion. Also reject a valid different manifest name
both at initial fallback admission and after teardown. Keep replacements intact
and report failure. The retained identity remains disabled when teardown already
completed; this is not successful uninstall.

Store registry installation paths as a BLOB scalar and expose them internally as
PathBuf. Unix encoding preserves native bytes; Windows encoding is UTF-16LE.
Native-path decoding is explicit and does not interpret display text as a path.
UI paths are presentation only. This adds no cross-OS database portability or
conversion for earlier unshipped prototype schemas.

Keep dotted MCP qualified names, but admit them once per discovery pass. User
keys reserve the namespace first; the first eligible plugin contribution wins
and later collisions are skipped with reasons. Status and execution consume the
same admission logic rather than collecting duplicate strings into a HashMap.

## Options considered

Blind deletion after teardown can delete a newly installed replacement. Following
canonical package targets erases installation ownership. Automatically moving
shared-root replacements to quarantine or adding crash recovery would add a
larger lifecycle contract; neither is needed for late revalidation.

A lossy TEXT path cannot represent native filesystem identity. JSON or base64 in
a textual location field would obscure the encoding without adding value over
one indivisible BLOB path scalar. Renaming all qualified server keys would break
the selected naming policy; detecting collisions preserves it without ambiguity
about which contribution was admitted.

## Consequences

Identity is checked after the known asynchronous gap; this is not a filesystem
sandbox or an atomic transaction against arbitrary hostile filesystem writers.
Registry, runtime, and filesystem operations still have explicit partial failure
outcomes. A changed entry can require a fresh remove request after inspection.
macOS filesystem restrictions can prevent creating arbitrary non-UTF-8 names;
byte-storage tests are independent of that restriction, and the full raw-name
lifecycle regression runs on Linux.
