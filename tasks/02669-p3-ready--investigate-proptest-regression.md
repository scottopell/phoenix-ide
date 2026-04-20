---
created: 2026-04-20
priority: p3
status: ready
artifact: proptest-regressions/state_machine/proptests.txt
---

# Investigate GraceTurnExhausted + ContextExhausted proptest regression

## Summary

C1 generated a proptest regression seed for a GraceTurnExhausted event reaching
a ContextExhausted state. The C1 agent handled it by absorbing the event in the
wrapper, but the edge case may indicate a real behavioral question about what
happens when a sub-agent-only event reaches a parent terminal state.

## Done When

Edge case understood. Either: the absorption is correct and documented, or a
real bug was found and fixed.
