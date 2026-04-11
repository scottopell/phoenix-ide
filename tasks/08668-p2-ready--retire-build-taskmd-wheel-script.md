---
created: 2026-04-11
priority: p2
status: ready
artifact: dev.py
---

# Retire build-taskmd-wheel.sh once taskmd is on PyPI

The `scripts/build-taskmd-wheel.sh` script and `.taskmd-wheel/` local cache exist
because `taskmd` is only available as a git source, which requires uv to build it
from scratch via maturin. maturin's pep517 stdout contamination bug causes that
build to fail — maturin writes tracing log lines to stdout, uv interprets them as
the wheel filename, and reports `FileNotFoundError`.

Once `taskmd` is published to PyPI with pre-built wheels, this workaround is
obsolete.

## When taskmd lands on PyPI

1. Change `dev.py` dependency back to `"taskmd"` (no git URL, no `[tool.uv]` block,
   no `find-links`):

   ```python
   # /// script
   # requires-python = ">=3.12"
   # dependencies = [
   #   "taskmd",
   # ]
   # ///
   ```

2. Remove the wheel directory check from `cmd_tasks_validate()` in `dev.py`.

3. Delete `scripts/build-taskmd-wheel.sh`.

4. Remove `.taskmd-wheel/` from `.gitignore`.

5. Update `README.md` Quick Start to remove the `build-taskmd-wheel.sh` step.

6. Close this task.

## Why the maturin bug happens

maturin's pep517 `build_wheel` hook is supposed to write only the wheel filename
to stdout. Instead it also writes structured tracing log lines (e.g.
`2026-04-10T... INFO pep517: maturin::commands::pep517: close time.busy=10.5s`).
uv reads that line as the wheel path, fails to open it, and reports a build failure
— even though the `.whl` was created successfully in the cargo target directory.

Pre-built wheels on PyPI bypass the build step entirely, so the bug is never hit.
