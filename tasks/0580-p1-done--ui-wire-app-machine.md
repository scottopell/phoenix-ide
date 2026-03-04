---
created: 2026-02-28
number: 580
priority: p1
status: done
slug: ui-wire-app-machine
title: "Wire appMachine.ts as live implementation via useAppMachine.ts"
---

# Wire App Machine

## Context

Read first:
- `specs/ui/design.md` — "Wiring appMachine.ts" section
- `specs/ui/design.md` — Appendix A (UI-FM-4)

UI-FM-4: `appMachine.ts` defines a correct pure FSM for online/offline/sync state.
`useAppMachine.ts` reimplements the same behavior with ad-hoc `useState`. Neither
imports the other. Both exist. The spec is dead code.

## What to Do

1. **Read both files** — `ui/src/machines/appMachine.ts` and `ui/src/hooks/useAppMachine.ts`.
   Understand the public interface of `useAppMachine` (what components consume from it).

2. **Rewrite `useAppMachine.ts` internals** to call `appMachine.ts`'s `transition()`
   function. Keep the public interface identical — zero call-site changes:

   ```typescript
   export function useAppMachine(): AppMachineHandle {
     const [appState, setAppState] = useState<AppState>(initialAppState);

     const dispatch = useCallback((event: AppEvent) => {
       setAppState(current => {
         const { state, effects } = transition(current, event, context);
         setTimeout(() => executeEffects(effects), 0);
         return state;
       });
     }, []);

     return {
       isReady: appState.type === 'ready',
       isOnline: /* derive from appState */,
       isSyncing: /* derive from appState */,
       pendingOpsCount: /* derive from appState */,
       queueOperation,
     };
   }
   ```

3. **Verify the public interface** is unchanged. Run `grep -r "useAppMachine"` to find
   all consumers. None should need changes.

4. **Test `appMachine.ts` in isolation** — import `transition`, feed events, assert
   outputs. No React, no DOM. If `appMachine.ts` has bugs found during this wiring,
   fix them in `appMachine.ts` (the spec), not in `useAppMachine.ts` (the adapter).

## Acceptance Criteria

- `useAppMachine.ts` imports and calls `transition` from `appMachine.ts`
- No ad-hoc `useState` for state that the machine tracks
- Public interface unchanged (grep call sites, verify no changes needed)
- `appMachine.ts` has at least one unit test exercising `transition()`
- App initializes correctly (manual test: load app, verify online/offline behavior)

## Dependencies

- None (independent of all other tasks)

## Files Likely Involved

- `ui/src/machines/appMachine.ts` — the pure FSM (read, possibly fix bugs)
- `ui/src/hooks/useAppMachine.ts` — the adapter (rewrite internals)
