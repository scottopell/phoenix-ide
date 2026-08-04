# Evaluate and gate macOS thermal scheduler enforcement

Use the shipped observe-only telemetry to establish numeric overhead and targeting baselines on supported Apple Silicon and Intel macOS hosts. Evaluate supported public nice/setpriority and QoS/task-policy controls with a fake native-policy boundary first, then real hosts. Do not add SIGSTOP, suspension, duty cycling, fan writes, or SMC access.

Enforcement remains disabled unless native ownership coverage is complete enough for the selected targets, sampling stays within documented CPU/memory/latency budgets, Phoenix remains responsive, workloads continue making progress, PID/start mismatches are safe no-ops, and observed evidence shows a useful pressure response. If gates fail, retain observe-only mode.

Acceptance evidence belongs in the implementation/PR and deployment-info executive status; add deterministic decision/effect tests and real-host coverage notes.
