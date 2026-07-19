import { describe, expect, it } from 'vitest';
import {
  activateFindSessionMatch,
  closeFindSession,
  createClosedFindSession,
  createMatchId,
  createSurfaceKey,
  nextFindSessionMatch,
  openFindSession,
  previousFindSessionMatch,
  replaceFindSessionResults,
  resetFindSession,
  setFindSessionQueryAndResults,
  type FindSessionCommand,
  type FindSessionMatch,
  type FindSessionState,
  type MatchId,
  type SearchableSurface,
} from './findSession';

type FocusOrigin = { scope: 'viewer'; token: string };
type Target = { ordinal: number; label: string };

function makeMatch(id: string, ordinal: number): FindSessionMatch<Target> {
  return { id: createMatchId(id), target: { ordinal, label: id } };
}

function makeSurface(
  options: Partial<SearchableSurface<Target, FocusOrigin>> & Pick<SearchableSurface<Target, FocusOrigin>, 'focusOrigin'>,
): SearchableSurface<Target, FocusOrigin> {
  return {
    key: options.key ?? createSurfaceKey('surface-a'),
    query: options.query ?? '',
    matches: options.matches ?? [],
    focusOrigin: options.focusOrigin,
  };
}

function expectReveal(
  command: FindSessionCommand<Target, FocusOrigin> | undefined,
  matchId: MatchId,
): void {
  expect(command).toEqual({
    kind: 'reveal-match',
    matchId,
    target: expect.objectContaining({ label: String(matchId) }),
  });
}

describe('findSession', () => {
  it('opens a closed session, focuses query, and reveals the first match', () => {
    const initial = createClosedFindSession<Target, FocusOrigin>();
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);

    const result = openFindSession(initial, makeSurface({
      query: 'a',
      matches: [alpha, beta],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    }));

    expect(result.state).toEqual({
      status: 'open',
      surfaceKey: createSurfaceKey('surface-a'),
      query: 'a',
      matches: [alpha, beta],
      activeMatchId: alpha.id,
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
      focusVersion: 1,
    });
    expect(result.commands).toEqual([
      { kind: 'focus-query', focusVersion: 1 },
      { kind: 'reveal-match', matchId: alpha.id, target: alpha.target },
    ]);
  });

  it('refocuses an already-open session without replacing focus origin', () => {
    const alpha = makeMatch('alpha', 0);
    const open = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [alpha],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    const result = openFindSession(open, makeSurface({
      query: 'different',
      matches: [],
      focusOrigin: { scope: 'viewer', token: 'origin-2' },
    }));

    expect(result.state.status).toBe('open');
    if (result.state.status !== 'open') throw new Error('expected open state');
    expect(result.state.focusOrigin).toEqual({ scope: 'viewer', token: 'origin-1' });
    expect(result.state.query).toBe('a');
    expect(result.state.focusVersion).toBe(2);
    expect(result.commands).toEqual([{ kind: 'focus-query', focusVersion: 2 }]);
  });

  it('closes an open session by clearing decorations and restoring focus', () => {
    const alpha = makeMatch('alpha', 0);
    const open = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [alpha],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    const result = closeFindSession(open);

    expect(result.state).toEqual({ status: 'closed' });
    expect(result.commands).toEqual([
      { kind: 'clear-decorations' },
      { kind: 'restore-focus', focusOrigin: { scope: 'viewer', token: 'origin-1' } },
    ]);
  });

  it('keeps closed state structurally empty', () => {
    const closed: FindSessionState<Target, FocusOrigin> = createClosedFindSession();
    expect(closed).toEqual({ status: 'closed' });
    expect('query' in closed).toBe(false);
    expect('matches' in closed).toBe(false);
    expect('activeMatchId' in closed).toBe(false);
    expect('focusOrigin' in closed).toBe(false);
  });

  it('set-query-and-results resets active match to the first result and clears empty queries', () => {
    const open = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [makeMatch('alpha', 0), makeMatch('beta', 1)],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    const narrowed = setFindSessionQueryAndResults(open, 'beta', [makeMatch('beta', 1)]);
    expect(narrowed.state.status).toBe('open');
    if (narrowed.state.status !== 'open') throw new Error('expected open state');
    expect(narrowed.state.activeMatchId).toBe(createMatchId('beta'));
    expect(narrowed.commands).toEqual([
      { kind: 'reveal-match', matchId: createMatchId('beta'), target: { ordinal: 1, label: 'beta' } },
    ]);

    const cleared = setFindSessionQueryAndResults(narrowed.state, '', [makeMatch('beta', 1)]);
    expect(cleared.state.status).toBe('open');
    if (cleared.state.status !== 'open') throw new Error('expected open state');
    expect(cleared.state.query).toBe('');
    expect(cleared.state.matches).toEqual([]);
    expect(cleared.state.activeMatchId).toBeNull();
    expect(cleared.commands).toEqual([{ kind: 'clear-decorations' }]);
  });

  it('replace-results preserves the active id when it still exists', () => {
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);
    const gamma = makeMatch('gamma', 2);
    let state = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [alpha, beta, gamma],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;
    state = activateFindSessionMatch(state, beta.id).state;

    const result = replaceFindSessionResults(state, [makeMatch('delta', 0), beta, makeMatch('epsilon', 2), gamma]);

    expect(result.state.status).toBe('open');
    if (result.state.status !== 'open') throw new Error('expected open state');
    expect(result.state.activeMatchId).toBe(beta.id);
    expect(result.commands).toEqual([]);
  });

  it('replace-results does not reveal a retained active match with an equal rebuilt target', () => {
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);
    let state = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [alpha, beta],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;
    state = activateFindSessionMatch(state, beta.id).state;

    const result = replaceFindSessionResults(state, [makeMatch('alpha', 0), makeMatch('beta', 1)]);

    expect(result.state.status).toBe('open');
    if (result.state.status !== 'open') throw new Error('expected open state');
    expect(result.state.activeMatchId).toBe(beta.id);
    expect(result.commands).toEqual([]);
  });

  it('replace-results reveals a retained active match when its target is refreshed', () => {
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);
    let state = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'a',
      matches: [alpha, beta],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;
    state = activateFindSessionMatch(state, beta.id).state;

    const movedBeta = makeMatch('beta', 8);
    const result = replaceFindSessionResults(state, [alpha, movedBeta]);

    expect(result.state.status).toBe('open');
    if (result.state.status !== 'open') throw new Error('expected open state');
    expect(result.state.activeMatchId).toBe(beta.id);
    expect(result.commands).toEqual([
      { kind: 'reveal-match', matchId: beta.id, target: movedBeta.target },
    ]);
  });

  it('replace-results falls back to the nearest prior ordinal on shrink, insertion, reorder, and removal', () => {
    const a = makeMatch('a', 0);
    const b = makeMatch('b', 1);
    const c = makeMatch('c', 2);
    const d = makeMatch('d', 3);
    let state = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'q',
      matches: [a, b, c, d],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;
    state = activateFindSessionMatch(state, c.id).state;

    const shrink = replaceFindSessionResults(state, [a, b]);
    expect(shrink.state.status).toBe('open');
    if (shrink.state.status !== 'open') throw new Error('expected open state');
    expect(shrink.state.activeMatchId).toBe(b.id);

    const insertion = replaceFindSessionResults(state, [a, makeMatch('x', 1), b, d]);
    expect(insertion.state.status).toBe('open');
    if (insertion.state.status !== 'open') throw new Error('expected open state');
    expect(insertion.state.activeMatchId).toBe(b.id);

    const reorder = replaceFindSessionResults(state, [d, a, b]);
    expect(reorder.state.status).toBe('open');
    if (reorder.state.status !== 'open') throw new Error('expected open state');
    expect(reorder.state.activeMatchId).toBe(b.id);

    const removal = replaceFindSessionResults(state, []);
    expect(removal.state.status).toBe('open');
    if (removal.state.status !== 'open') throw new Error('expected open state');
    expect(removal.state.activeMatchId).toBeNull();
    expect(removal.commands).toEqual([{ kind: 'clear-decorations' }]);
  });

  it('next and previous wrap around the ordered result list', () => {
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);
    const gamma = makeMatch('gamma', 2);
    let state = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'q',
      matches: [alpha, beta, gamma],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    state = previousFindSessionMatch(state).state;
    expect(state.status).toBe('open');
    if (state.status !== 'open') throw new Error('expected open state');
    expect(state.activeMatchId).toBe(gamma.id);

    state = nextFindSessionMatch(state).state;
    expect(state.status).toBe('open');
    if (state.status !== 'open') throw new Error('expected open state');
    expect(state.activeMatchId).toBe(alpha.id);
  });

  it('activate ignores unknown ids and reveals known ids', () => {
    const alpha = makeMatch('alpha', 0);
    const beta = makeMatch('beta', 1);
    const open = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'q',
      matches: [alpha, beta],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    const ignored = activateFindSessionMatch(open, createMatchId('missing'));
    expect(ignored.state).toBe(open);
    expect(ignored.commands).toEqual([]);

    const activated = activateFindSessionMatch(open, beta.id);
    expect(activated.state.status).toBe('open');
    if (activated.state.status !== 'open') throw new Error('expected open state');
    expect(activated.state.activeMatchId).toBe(beta.id);
    expectReveal(activated.commands[0], beta.id);
  });

  it('reset emits only clear-decorations and returns closed', () => {
    const result = resetFindSession<Target, FocusOrigin>();
    expect(result.state).toEqual({ status: 'closed' });
    expect(result.commands).toEqual([{ kind: 'clear-decorations' }]);
  });

  it('surface replacement structurally replaces focus origin, query, and results', () => {
    const open = openFindSession(createClosedFindSession<Target, FocusOrigin>(), makeSurface({
      query: 'q',
      matches: [makeMatch('alpha', 0)],
      focusOrigin: { scope: 'viewer', token: 'origin-1' },
    })).state;

    const { state, commands } = closeFindSession(open);
    const reopened = openFindSession(state, makeSurface({
      key: createSurfaceKey('surface-b'),
      query: 'z',
      matches: [makeMatch('zeta', 0)],
      focusOrigin: { scope: 'viewer', token: 'origin-2' },
    }));

    expect(reopened.state.status).toBe('open');
    if (reopened.state.status !== 'open') throw new Error('expected open state');
    expect(reopened.state.surfaceKey).toBe(createSurfaceKey('surface-b'));
    expect(reopened.state.focusOrigin).toEqual({ scope: 'viewer', token: 'origin-2' });
    expect(reopened.state.query).toBe('z');
    expect(commands).toEqual([
      { kind: 'clear-decorations' },
      { kind: 'restore-focus', focusOrigin: { scope: 'viewer', token: 'origin-1' } },
    ]);
  });

  it('encodes invalid states as type errors', () => {
    const closed = createClosedFindSession<Target, FocusOrigin>();
    // @ts-expect-error closed sessions do not carry query state
    void closed.query;

    // @ts-expect-error closed sessions cannot be constructed with open-state fields
    const impossibleState: FindSessionState<Target, FocusOrigin> = { status: 'closed', query: 'x' };
    expect((impossibleState as { status: 'closed' }).status).toBe('closed');
  });
});
