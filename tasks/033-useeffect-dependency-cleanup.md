---
created: 2026-02-02
priority: p3
status: ready
---

# Clean Up useEffect Dependency Patterns

## Summary

The `useConnection` hook and `ConversationPage` have some useEffect calls that intentionally omit dependencies to avoid infinite loops. This works but ESLint would complain, and it's not the cleanest pattern.

## Current Pattern (Problematic)

```typescript
// In ConversationPage.tsx
useEffect(() => {
  if (conversationId) {
    seenIdsRef.current.clear();
    dispatch({ type: 'CONNECT' });
  } else {
    dispatch({ type: 'DISCONNECT' });
  }

  return () => {
    dispatch({ type: 'DISCONNECT' });
  };
}, [conversationId, dispatch]);  // dispatch changes each render if executeEffects changes
```

The issue: `dispatch` depends on `executeEffects` which depends on `updateSequenceId` which can change. This creates unnecessary re-connections.

## Recommended Pattern: Stable Callback Ref

```typescript
// Keep a ref to the latest callback
const dispatchRef = useRef(dispatch);
useEffect(() => { dispatchRef.current = dispatch; });

// Use the ref in effects that shouldn't re-run when callback changes
useEffect(() => {
  if (conversationId) {
    seenIdsRef.current.clear();
    dispatchRef.current({ type: 'CONNECT' });
  } else {
    dispatchRef.current({ type: 'DISCONNECT' });
  }

  return () => {
    dispatchRef.current({ type: 'DISCONNECT' });
  };
}, [conversationId]);  // Only depends on conversationId now
```

## Alternative: React 18.3+ useEffectEvent

Once available:
```typescript
const onConnect = useEffectEvent(() => {
  dispatch({ type: 'CONNECT' });
});

useEffect(() => {
  if (conversationId) onConnect();
}, [conversationId]);
```

## Acceptance Criteria

- [ ] No ESLint warnings about missing dependencies
- [ ] Callbacks don't cause unnecessary re-execution of effects
- [ ] Pattern is consistent across hooks

## Affected Files

- `ui/src/hooks/useConnection.ts`
- `ui/src/pages/ConversationPage.tsx`
