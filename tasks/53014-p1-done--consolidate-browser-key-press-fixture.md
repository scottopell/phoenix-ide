# Consolidate browser key-press fixture

Run Escape, Ctrl-modified capture, ArrowDown focus, and unknown-key rejection scenarios through one panic-safe serialized browser fixture. Reset event logs, focus, signals, and modifier state; capture five samples, launch counts, fault proof, broad validation, and a separate PR.

## Outcome

Rejected after measurement. The four isolated tests launch Chrome four times, but the CDP Ctrl+K contract consumes roughly 33–41 CPU-seconds per run on this host. In a shared-session candidate, Ctrl+K reached the capture listener but both repeated runs left the following browser eval timing out after 15 seconds; the candidate failed after about 52.6 seconds each time. Switching Ctrl+K to JavaScript dispatch would violate REQ-BT-016's CDP-level modifier contract, so no behavior-preserving 4-to-1 fixture was found. The candidate source was restored and no browser-test implementation change ships.
