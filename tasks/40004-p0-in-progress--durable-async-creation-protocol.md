# Durable async conversation creation protocol

Replace best-effort async conversation provisioning with a rigorously specified, deterministic-simulation-tested protocol: atomic claims and generations, durable leases and bounded retries, fence-inspect-resume reconciliation, typed resource ownership, first-class runtime bootstrap, and distinct cancel/delete cleanup semantics. Keep async creation unmerged until protocol, SQLite, Git, worker, UI recovery, and end-to-end gates pass.
