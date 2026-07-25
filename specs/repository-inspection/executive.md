# Repository Inspection

## Purpose

Repository Inspection gives Coordinator and authorized Restricted conversations a structured, bounded evidence path for local branch and commit compatibility triage without granting shell or Work authority.

## Current status

Implementation is tracked by REQ-RI-001 through REQ-RI-008. The first slice covers authoritative target resolution plus local status, log, diff, committed file reads, and committed-tree search. Network pull-request metadata remains excluded.

## Verification surfaces

| Surface | Coverage |
| --- | --- |
| Target and authority resolution | Resolver integration tests |
| Structured Git operations and evidence | Operation tests with temporary repositories |
| Mutation and escape resistance | Adversarial argv, config, traversal, symlink, timeout, and bound tests |
| Coordinator tool registration | Registry/runtime tests |
