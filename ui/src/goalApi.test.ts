import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as v from 'valibot';
import { controlSchema, GoalApiError, goalApi, goalEntrySchema, goalSource, goalStateLabel, goalStateReason } from './goalApi';
import type { GoalCommand, GoalDisposition, GoalEntry } from './goalApi';

const id = '11111111-1111-4111-8111-111111111111';
const principal = '22222222-2222-4222-8222-222222222222';
function fact(owner = 'pc-1', version = 1): GoalEntry {
  const actor = { UserControl: { principal_id: principal, request_id: id, command: 'create' as const } };
  return {
    snapshot: { owner, goal_id: 'goal-1', version, objective: '  exact objective  ', state: 'Underway' },
    mutation: { owner, expected_version: version - 1, action_id: 'user-command-1', attribution: { requested_by: actor, applied_by: actor }, change: { Create: { goal_id: 'goal-1', objective: '  exact objective  ' } } },
    recorded_at_us: 123,
  };
}
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
const fetchMock = vi.fn<typeof fetch>();
beforeEach(() => { fetchMock.mockReset(); vi.stubGlobal('fetch', fetchMock); });
afterEach(() => { vi.unstubAllGlobals(); });

describe('goal wire contract', () => {
  it('preserves actual externally tagged Rust actors, states, changes, timestamp and source facts', () => {
    const source = { product_conversation_id: 'pc-coordinator', transcript_id: 'tx-2', execution: { workflow_id: 7, turn_id: 8 }, message_id: 'message-9', tool_use_id: 'tool-10' };
    const actors = [fact().mutation.attribution.requested_by,
      { UserApi: { transcript_id: 'tx-1', message_id: 'message-1' } },
      { OwningAgent: source }, { Coordinator: source }, { GeneratedHost: { action_id: 'host-1' } }];
    for (const actor of actors) {
      const raw = { ...fact(), mutation: { ...fact().mutation, attribution: { requested_by: actor, applied_by: actor } } };
      const parsed = v.parse(goalEntrySchema, raw);
      expect(parsed).toEqual(raw);
      expect(goalSource(parsed)).not.toBe('');
    }
    for (const state of ['Underway', { Blocked: 'need access' }, { Suspended: 'replaced by goal-2' }, { Complete: 'tests verified' }, { Exhausted: 'budget used' }]) {
      const raw = { ...fact(), snapshot: { ...fact().snapshot, state }, mutation: { ...fact().mutation, change: { SetDisposition: { goal_id: 'goal-1', disposition: state } } } };
      const parsed = v.parse(goalEntrySchema, raw);
      expect(parsed).toEqual(raw);
      expect(goalStateLabel(parsed)).not.toBe('');
      expect(goalStateReason(parsed)).toEqual(state === 'Underway' ? null : Object.values(state)[0]);
    }
    const report = { ...fact(), mutation: { ...fact().mutation, change: { Report: { binding: { owner: 'pc-1', execution: source.execution, observation: { Goal: { owner: 'pc-1', goal_id: 'goal-1', version: 1 } } }, disposition: { Blocked: 'report facts' } } } } };
    expect(v.parse(goalEntrySchema, report)).toEqual(report);
    const emptyObservation = { ...report, mutation: { ...report.mutation, change: { Report: { ...report.mutation.change.Report, binding: { ...report.mutation.change.Report.binding, observation: { NoGoal: { observed_version: 0 } } } } } } };
    expect(v.parse(goalEntrySchema, emptyObservation)).toEqual(emptyObservation);
  });

  it('fails closed for unsafe counters, fabricated wire encodings, omitted fields and empty reasons', () => {
    expect(v.safeParse(controlSchema, { version: 0, stopped: false }).success).toBe(false);
    expect(v.safeParse(controlSchema, { version: Number.MAX_SAFE_INTEGER + 1, stopped: false }).success).toBe(false);
    for (const state of [{ state: 'blocked', reason: 'wrong encoding' }, { Blocked: ' ' }, { Abandoned: 'not a Rust variant' }]) {
      expect(v.safeParse(goalEntrySchema, { ...fact(), snapshot: { ...fact().snapshot, state } }).success).toBe(false);
    }
    expect(v.safeParse(goalEntrySchema, { ...fact(), recorded_at: 123 }).success).toBe(false);
    expect(v.safeParse(goalEntrySchema, { ...fact(), snapshot: { ...fact().snapshot, version: 1.5 } }).success).toBe(false);
    expect(v.safeParse(goalEntrySchema, { ...fact(), recorded_at_us: Number.MAX_SAFE_INTEGER + 1 }).success).toBe(false);
    const unsafeActor = { OwningAgent: { product_conversation_id: 'pc-1', transcript_id: 'tx-1', execution: { workflow_id: 1, turn_id: Number.MAX_SAFE_INTEGER + 1 }, message_id: 'm-1', tool_use_id: 't-1' } };
    expect(v.safeParse(goalEntrySchema, { ...fact(), mutation: { ...fact().mutation, attribution: { requested_by: unsafeActor, applied_by: unsafeActor } } }).success).toBe(false);
  });

  it('reads only the canonical encoded owner and distinguishes null absence from disabled 503', async () => {
    fetchMock.mockResolvedValueOnce(json(null)).mockResolvedValueOnce(json(null)).mockResolvedValueOnce(json({ error: 'disabled' }, 503));
    await expect(goalApi.current('pc/1')).resolves.toBeNull();
    await expect(goalApi.control('pc/1')).resolves.toBeNull();
    expect(fetchMock.mock.calls[0]![0]).toBe('/api/product-conversations/pc%2F1/goal');
    await expect(goalApi.current('pc/1')).rejects.toMatchObject({ status: 503, outcome: 'known' });
  });

  it('rejects foreign owner, oversized or unordered history and unsafe cursors', async () => {
    fetchMock.mockResolvedValueOnce(json(fact('foreign')));
    await expect(goalApi.current('pc-1')).rejects.toThrow('owner mismatch');
    fetchMock.mockResolvedValueOnce(json([fact('pc-1', 2), fact('pc-1', 1)]));
    await expect(goalApi.history('pc-1', 0)).rejects.toThrow('Unordered');
    fetchMock.mockResolvedValueOnce(json([fact('pc-1', 1), fact('pc-1', 1)]));
    await expect(goalApi.history('pc-1', 0)).rejects.toThrow('Unordered');
    fetchMock.mockResolvedValueOnce(json([fact('foreign', 1)]));
    await expect(goalApi.history('pc-1', 0)).rejects.toThrow('owner mismatch');
    fetchMock.mockResolvedValueOnce(json(Array.from({ length: 101 }, (_, index) => fact('pc-1', index + 1))));
    await expect(goalApi.history('pc-1', 0)).rejects.toThrow();
    fetchMock.mockResolvedValueOnce(json([fact('pc-1', 100)]));
    await expect(goalApi.history('pc-1', 100)).rejects.toThrow('Unordered');
    const before = fetchMock.mock.calls.length;
    await expect(goalApi.history('pc-1', -1)).rejects.toThrow();
    expect(fetchMock).toHaveBeenCalledTimes(before);
  });
});

describe('explicit command receipts', () => {
  const command: GoalCommand = { kind: 'create', body: { request_id: id, expected_version: 0, objective: '  exact objective  ' } };
  it('sends only explicit DTO fields, accepts factual receipt, and never sends chat or consent', async () => {
    fetchMock.mockResolvedValueOnce(json({ request_id: id, result: { outcome: 'accepted', entry: fact() } }));
    await expect(goalApi.command('pc-1', command)).resolves.toContain('Command recorded');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, options] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/product-conversations/pc-1/goal');
    expect(JSON.parse(String(options?.body))).toEqual(command.body);
  });

  it('retains unknown classification for network, 5xx and malformed/mismatched success receipts', async () => {
    fetchMock.mockRejectedValueOnce(new Error('lost response'));
    await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ outcome: 'unknown' });
    fetchMock.mockResolvedValueOnce(json({ error: 'internal' }, 500));
    await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ outcome: 'unknown' });
    fetchMock.mockResolvedValueOnce(json({ request_id: principal, result: { outcome: 'accepted', entry: fact() } }));
    await expect(goalApi.command('pc-1', command)).rejects.toBeInstanceOf(GoalApiError);
    fetchMock.mockResolvedValueOnce(json({ request_id: id, result: { outcome: 'accepted', entry: fact('foreign') } }));
    await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ outcome: 'unknown' });
  });

  it('handles known refusals/auth/uninstalled distinctly without claiming success', async () => {
    fetchMock.mockResolvedValueOnce(json({ request_id: id, result: { outcome: 'rejected', reason: 'busy', observed_version: 0, recorded_at: 123 } }, 409));
    await expect(goalApi.command('pc-1', command)).resolves.toContain('refused: busy');
    for (const status of [401, 403]) {
      fetchMock.mockResolvedValueOnce(new Response(null, { status }));
      await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ status, outcome: 'known' });
    }
    for (const [status, error] of [[409, 'goal command admission changed or is fenced'], [503, 'goal services are not installed']] as const) {
      fetchMock.mockResolvedValueOnce(json({ error }, status));
      await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ status, outcome: 'known' });
    }
  });

  it('does not mistake arbitrary 503/409 bodies or unexpected success statuses for definite refusal', async () => {
    for (const status of [503, 409, 502, 504, 202]) {
      fetchMock.mockResolvedValueOnce(json({ error: 'upstream response unavailable' }, status));
      await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ status, outcome: 'unknown' });
    }
    fetchMock.mockResolvedValueOnce(new Response('truncated', { status: 503 }));
    await expect(goalApi.command('pc-1', command)).rejects.toMatchObject({ status: 503, outcome: 'unknown' });
  });

  it.each(['underway', 'blocked', 'complete', 'exhausted'] as const)('correlates the exact State disposition and snapshot (%s)', async (state) => {
    const reason = '  exact reason  ';
    const disposition: GoalDisposition = state === 'underway' ? { state } : { state, reason };
    const wireState: GoalEntry['snapshot']['state'] = state === 'underway' ? 'Underway' : state === 'blocked' ? { Blocked: reason }
      : state === 'complete' ? { Complete: reason } : { Exhausted: reason };
    const actor = { UserControl: { principal_id: principal, request_id: id, command: 'state' as const } };
    const entry: GoalEntry = { ...fact('pc-1', 2), snapshot: { ...fact('pc-1', 2).snapshot, state: wireState },
      mutation: { ...fact('pc-1', 2).mutation, attribution: { requested_by: actor, applied_by: actor },
        change: { SetDisposition: { goal_id: 'goal-1', disposition: wireState } } } };
    const stateCommand: GoalCommand = { kind: 'state', body: { request_id: id, expected_version: 1, goal_id: 'goal-1', disposition } };
    fetchMock.mockResolvedValueOnce(json({ request_id: id, result: { outcome: 'accepted', entry } }));
    await expect(goalApi.command('pc-1', stateCommand)).resolves.toContain('Command recorded');
    expect(fetchMock.mock.lastCall?.[0]).toBe('/api/product-conversations/pc-1/goal/state');
    expect(JSON.parse(String(fetchMock.mock.lastCall?.[1]?.body))).toEqual(stateCommand.body);
    for (const corrupted of [
      { ...entry, snapshot: { ...entry.snapshot, state: { Blocked: 'different reason' } } },
      { ...entry, snapshot: { ...entry.snapshot, goal_id: 'foreign-goal' } },
      { ...entry, mutation: { ...entry.mutation, change: { SetDisposition: { goal_id: 'goal-1', disposition: { Blocked: 'different reason' } } } } },
      { ...entry, mutation: { ...entry.mutation, attribution: { requested_by: actor, applied_by: { GeneratedHost: { action_id: 'not this request' } } } } },
    ]) {
      fetchMock.mockResolvedValueOnce(json({ request_id: id, result: { outcome: 'accepted', entry: corrupted } }));
      await expect(goalApi.command('pc-1', stateCommand)).rejects.toMatchObject({ outcome: 'unknown' });
    }
  });

  it('Stop validates the actual principal/CAS receipt without inferring from goal state', async () => {
    const stop: GoalCommand = { kind: 'stop', body: { request_id: id, command: 'stop', expected_control_version: 0 } };
    const receipt = { owner: 'pc-1', principal_id: principal, request_id: id, command: 'stop', expected_control_version: 0, result: { outcome: 'accepted', snapshot: { version: 1, stopped: true } }, recorded_at: 123 };
    fetchMock.mockResolvedValueOnce(json(receipt));
    await expect(goalApi.command('pc-1', stop)).resolves.toContain('Stop recorded');
    expect(fetchMock.mock.calls[0]![0]).toBe('/api/product-conversations/pc-1/goal-control');
    fetchMock.mockResolvedValueOnce(json({ ...receipt, expected_control_version: 5 }));
    await expect(goalApi.command('pc-1', stop)).rejects.toMatchObject({ outcome: 'unknown' });
  });
});
