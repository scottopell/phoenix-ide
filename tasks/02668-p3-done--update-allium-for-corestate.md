---
created: 2026-04-20
priority: p3
status: done
artifact: specs/bedrock/bedrock.allium
---

# Update bedrock.allium for CoreState wrapper pattern

## Summary

bedrock.allium still describes a single ConvState enum. The code now has
CoreState + ParentState + SubAgentState. The spec should reflect the
structural split so future spec reviews catch divergence.

## Done When

bedrock.allium entity section describes the wrapper pattern.
Transition rules reference the split functions.
