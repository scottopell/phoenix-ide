import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { GoalApiError, goalApi } from '../goalApi';
import type { GoalCommand, GoalControl, GoalEntry } from '../goalApi';

type Facts =
  | { owner: string | null; kind: 'loading' }
  | { owner: string; kind: 'unavailable' | 'error'; message: string }
  | { owner: string; kind: 'ready'; current: GoalEntry | null; control: GoalControl | null; history: GoalEntry[]; more: boolean };
type Pending = { command: GoalCommand; outcome: 'sending' | 'unknown' };

export function useConversationGoals(owner: string | null, writable: boolean) {
  const [facts, setFacts] = useState<Facts>({ owner, kind: 'loading' });
  const [notice, setNotice] = useState<{ owner: string; text: string } | null>(null);
  const [busyHistory, setBusyHistory] = useState(false);
  const [, rerenderPending] = useState(0);
  const pending = useRef(new Map<string, Pending>());
  const active = useRef(owner);
  const generation = useRef(0);
  const ownerEpoch = useRef(0);
  const historyRequest = useRef<object | null>(null);
  useLayoutEffect(() => {
    active.current = owner;
    generation.current += 1;
    ownerEpoch.current += 1;
    return () => { active.current = null; generation.current += 1; ownerEpoch.current += 1; };
  }, [owner]);

  const refresh = useCallback(async () => {
    if (!owner || active.current !== owner) return;
    const ticket = ++generation.current;
    setFacts({ owner, kind: 'loading' });
    setBusyHistory(false);
    historyRequest.current = null;
    try {
      const [current, control, history] = await Promise.all([
        goalApi.current(owner), goalApi.control(owner), goalApi.history(owner, 0),
      ]);
      if (active.current === owner && generation.current === ticket) {
        setFacts({ owner, kind: 'ready', current, control, history, more: history.length === 100 });
      }
    } catch (error) {
      if (active.current !== owner || generation.current !== ticket) return;
      setFacts({ owner, kind: error instanceof GoalApiError && error.status === 503 ? 'unavailable' : 'error',
        message: error instanceof Error ? error.message : 'Could not read goal facts.' });
    }
  }, [owner]);
  useEffect(() => { void refresh(); }, [refresh]);

  const displayed = facts.owner === owner ? facts : { owner, kind: 'loading' as const };
  const unresolved = owner ? pending.current.get(owner) : undefined;
  const canWrite = writable && displayed.kind === 'ready' && !unresolved;
  const submit = async (command: GoalCommand, retry = false) => {
    if (!owner || active.current !== owner || !writable) return;
    const previous = pending.current.get(owner);
    if (retry ? previous?.outcome !== 'unknown' : !canWrite || previous) return;
    const retained = retry && previous ? previous.command : command;
    const epoch = ownerEpoch.current;
    pending.current.set(owner, { command: retained, outcome: 'sending' });
    rerenderPending((n) => n + 1);
    try {
      const result = await goalApi.command(owner, retained);
      if (ownerEpoch.current !== epoch || active.current !== owner) {
        pending.current.set(owner, { command: retained, outcome: 'unknown' });
        return;
      }
      pending.current.delete(owner);
      if (active.current === owner) {
        setNotice({ owner, text: result });
        await refresh();
      }
    } catch (error) {
      const sameOwnerEpoch = ownerEpoch.current === epoch && active.current === owner;
      const unknown = retry || !sameOwnerEpoch || !(error instanceof GoalApiError) || error.outcome === 'unknown';
      if (unknown) pending.current.set(owner, { command: retained, outcome: 'unknown' });
      else pending.current.delete(owner);
      if (sameOwnerEpoch) {
        setNotice({ owner, text: error instanceof Error ? error.message : 'Command outcome unknown. Retry this exact request.' });
        if (!unknown) await refresh();
      }
    } finally {
      if (active.current === owner) rerenderPending((n) => n + 1);
    }
  };
  const moreHistory = async () => {
    if (!owner || active.current !== owner || displayed.kind !== 'ready' || historyRequest.current || !displayed.more) return;
    const request = {};
    historyRequest.current = request;
    const ticket = generation.current;
    const after = displayed.history.at(-1)?.snapshot.version ?? 0;
    setBusyHistory(true);
    try {
      const page = await goalApi.history(owner, after);
      if (active.current !== owner || generation.current !== ticket) return;
      setFacts((value) => value.kind === 'ready' && value.owner === owner
        ? { ...value, history: [...value.history, ...page], more: page.length === 100 } : value);
    } catch (error) {
      if (active.current === owner && generation.current === ticket) {
        setFacts({ owner, kind: 'error', message: error instanceof Error ? error.message : 'Could not read history.' });
      }
    } finally {
      if (historyRequest.current === request) historyRequest.current = null;
      if (active.current === owner && generation.current === ticket) setBusyHistory(false);
    }
  };
  return { facts: displayed, canWrite, writable, unresolved, refresh, submit, moreHistory, busyHistory,
    notice: notice?.owner === owner ? notice.text : null };
}
