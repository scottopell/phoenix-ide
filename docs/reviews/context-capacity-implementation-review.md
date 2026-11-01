# Context capacity implementation reviews

Two fresh reviewers independently challenged the complete implementation against normative requirements and Allium. Neither received the other review's conclusions. The earlier specification review counts as neither round.

## Round one

Frozen range: `9ebd9d8d42876c3356557f4ab461b3b8ffabd743..3fe4b565a53228c0d71a483fb9cd3ae53cc945f8`.

Two P2 findings survived source-level counterevidence checks:

1. The first successor's budget projection reused root transcript topology and omitted the recall orientation that the actual successor receives. Repair `3ab36bdc0` shares authenticated successor orientation with actual request assembly. A boundary regression compares projected and actual instructions, tools, overhead and retained working room.
2. Model-upgrade runtime replacement discarded the known model-switch diagnostic cause. The same repair carries a one-shot known cause into the replacement runtime while retaining unknown prefix comparison. Actual endpoint coverage also verifies that a same-model settings update does not invent a model-switch cause.

Parent validation passed the new boundary regression, all 16 transcript-recall tests, all seven model-switch tests and the prefix-cause regression. The review itself did not run Cargo or production operations.

## Round two

Frozen range: `9ebd9d8d42876c3356557f4ab461b3b8ffabd743..3ab36bdc0b3d7ae15ae44ed7e53b0abd2f562ab3`.

**No actionable findings.** The independent reviewer verified the complete binary patch and 96-file inventory, then traced capacity, counting, retention, handoff authority, recovery/restart, model switching, diagnostics correlation, UI and deployment boundaries. Nine disposable source129-to-target131 deployment tests passed. No Cargo build, live provider call or production operation was performed.

The reviewer specifically falsified suspected automatic-continuation restart, seeded-request pressure, forward-recovery claim-release and superseded-count correlation defects by tracing their downstream owners.

## Scope and remaining gates

The rounds have different heads and are not an exact-target comparison. Round two includes the round-one repairs and the historical systemd fixture correction. Reports were sealed before conclusions were shared; their original full reports are retained in the originating harness. Subsequent documentation-only changes do not constitute another implementation review.

The parent owns full `./dev.py check --all`, hosted CI, PR acceptance, native deployment and supported production recovery. Review does not prove provider count availability/latency, cache residency, lossless semantic summarization or production acceptance.
