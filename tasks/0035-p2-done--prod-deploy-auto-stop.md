---
created: 2025-02-02
priority: p2
status: done
---

# Auto-stop service during prod deploy

## Summary

`./dev.py prod deploy` should automatically stop the running service before copying the new binary, rather than failing with a "text file busy" error.

## Context

Currently, if the production service is running, `prod deploy` fails when trying to copy the new binary because the file is in use:

```
subprocess.CalledProcessError: Command '['sudo', 'cp', ...]' returned non-zero exit status 1.
```

The workaround is to manually run `./dev.py prod stop` first, but this should be automatic.

## Acceptance Criteria

- [x] `prod deploy` stops the service before copying the binary (if running)
- [x] Service is restarted after successful copy
- [x] If copy fails, provide clear error message
- [x] If service wasn't running before, start it anyway (deploy implies wanting it running)

## Notes

The fix is simple - add a stop step in `cmd_prod_deploy()` before the copy:

```python
# Stop service if running (binary may be in use)
subprocess.run(["sudo", "systemctl", "stop", PROD_SERVICE_NAME], capture_output=True)
```
