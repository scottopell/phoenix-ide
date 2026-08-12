# Roadmap reducer comment acknowledgment reactions

Structured Issue #651 roadmap comments need an observable processing lifecycle. The reducer must mark a trusted structured comment with eyes while processing, replace that with rocket only after the exact update is projected (or retirement applied), and replace it with confused when validation or application rejects the record. Agent guidance must poll these reactions, verify the generated snapshot/source on acceptance, and inspect the reducer Actions log on rejection.
