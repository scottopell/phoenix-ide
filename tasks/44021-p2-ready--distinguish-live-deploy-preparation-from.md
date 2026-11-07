# Distinguish live deployment preparation from stale recovery in prod status

During ordinary local-HEAD launchd deployment from merged commit 3b602a0b6 (#977), prod status displayed a RECOVERY warning about missing mode evidence while the live preparer was still building the candidate, before the manifest existed. It subsequently displayed STALE during the live read-only database integrity gate. The transaction ultimately committed successfully without intervention; the production runtime remained available during preparation.

Reproduce with a cold ordinary release build and a substantial production-like SQLite fixture, polling prod status during build and integrity validation. Do not experiment on a live production database to reproduce this.

Make the display distinguish an actively owned preparing/prepared operation from an orphaned transaction. Preserve fail-closed manifest validation, helper identity/absence checks, ownership fences, and all existing recovery restrictions. This is a diagnostic-quality fix, not permission to widen recovery or remove claims.

Validate live preparer before manifest creation, live helper during a slow integrity gate, dead preparer/helper, invalid evidence, and actual stale recovery. Retain the separate PTY descriptor-inheritance investigation and local native-launcher bootstrap tasks.
