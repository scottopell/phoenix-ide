export type MatchId = string & { readonly __brand: 'ViewerFindMatchId' };
export type SearchableSurfaceKey = string & { readonly __brand: 'ViewerFindSurfaceKey' };

export interface FindSessionMatch<TTarget> {
  id: MatchId;
  target: TTarget;
}

export interface SearchableSurface<TTarget, TFocusOrigin> {
  key: SearchableSurfaceKey;
  query: string;
  matches: readonly FindSessionMatch<TTarget>[];
  focusOrigin: TFocusOrigin;
}

export interface FindSessionClosedState {
  status: 'closed';
}

export interface FindSessionOpenState<TTarget, TFocusOrigin> {
  status: 'open';
  surfaceKey: SearchableSurfaceKey;
  query: string;
  matches: readonly FindSessionMatch<TTarget>[];
  activeMatchId: MatchId | null;
  focusOrigin: TFocusOrigin;
  focusVersion: number;
}

export type FindSessionState<TTarget, TFocusOrigin> =
  | FindSessionClosedState
  | FindSessionOpenState<TTarget, TFocusOrigin>;

export type FindSessionCommand<TTarget, TFocusOrigin> =
  | { kind: 'focus-query'; focusVersion: number }
  | { kind: 'restore-focus'; focusOrigin: TFocusOrigin }
  | { kind: 'reveal-match'; matchId: MatchId; target: TTarget }
  | { kind: 'clear-decorations' };

export type FindSessionAction<TTarget, TFocusOrigin> =
  | { type: 'open'; surface: SearchableSurface<TTarget, TFocusOrigin> }
  | { type: 'close' }
  | { type: 'set-query'; query: string }
  | { type: 'set-query-and-results'; query: string; matches: readonly FindSessionMatch<TTarget>[] }
  | { type: 'replace-results'; matches: readonly FindSessionMatch<TTarget>[] }
  | { type: 'next' }
  | { type: 'previous' }
  | { type: 'activate'; matchId: MatchId }
  | { type: 'reset' }
  | { type: 'replace-surface'; surface: SearchableSurface<TTarget, TFocusOrigin> };

export interface ReduceFindSessionResult<TTarget, TFocusOrigin> {
  state: FindSessionState<TTarget, TFocusOrigin>;
  commands: readonly FindSessionCommand<TTarget, TFocusOrigin>[];
}

export function createMatchId(value: string): MatchId {
  return value as MatchId;
}

export function createSurfaceKey(value: string): SearchableSurfaceKey {
  return value as SearchableSurfaceKey;
}

export function createClosedFindSession<TTarget, TFocusOrigin>(): FindSessionState<TTarget, TFocusOrigin> {
  return { status: 'closed' };
}

export function reduceFindSession<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  action: FindSessionAction<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  switch (action.type) {
    case 'open':
      return openFindSession(state, action.surface);
    case 'close':
      return closeFindSession(state);
    case 'set-query':
      return setFindSessionQuery(state, action.query);
    case 'set-query-and-results':
      return setFindSessionQueryAndResults(state, action.query, action.matches);
    case 'replace-results':
      return replaceFindSessionResults(state, action.matches);
    case 'next':
      return nextFindSessionMatch(state);
    case 'previous':
      return previousFindSessionMatch(state);
    case 'activate':
      return activateFindSessionMatch(state, action.matchId);
    case 'reset':
      return resetFindSession();
    case 'replace-surface':
      return replaceFindSessionSurface(state, action.surface);
    default:
      return { state, commands: [] };
  }
}

export function openFindSession<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  surface: SearchableSurface<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'open' && state.surfaceKey === surface.key) {
    return {
      state: { ...state, focusVersion: state.focusVersion + 1 },
      commands: [{ kind: 'focus-query', focusVersion: state.focusVersion + 1 }],
    };
  }

  const nextState = createOpenState(surface, 1);
  return {
    state: nextState,
    commands: [
      { kind: 'focus-query', focusVersion: nextState.focusVersion },
      ...revealCommand(nextState),
    ],
  };
}

export function closeFindSession<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return { state, commands: [] };
  return {
    state: createClosedFindSession(),
    commands: [
      { kind: 'clear-decorations' },
      { kind: 'restore-focus', focusOrigin: state.focusOrigin },
    ],
  };
}

export function setFindSessionQuery<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  query: string,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return { state, commands: [] };
  return {
    state: { ...state, query, matches: [], activeMatchId: null },
    commands: [{ kind: 'clear-decorations' }],
  };
}

export function setFindSessionQueryAndResults<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  query: string,
  matches: readonly FindSessionMatch<TTarget>[],
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return { state, commands: [] };
  const nextState = reconcileOpenState(state, query, matches, null);
  return {
    state: nextState,
    commands: nextState.activeMatchId === null ? [{ kind: 'clear-decorations' }] : revealCommand(nextState),
  };
}

export function replaceFindSessionResults<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  matches: readonly FindSessionMatch<TTarget>[],
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return { state, commands: [] };
  const previousActiveMatch = state.matches.find((match) => match.id === state.activeMatchId);
  const nextState = reconcileOpenState(state, state.query, matches, state.activeMatchId);
  const nextActiveMatch = nextState.matches.find((match) => match.id === nextState.activeMatchId);
  const activeMatchChanged = nextState.activeMatchId !== state.activeMatchId;
  const activeTargetChanged = !sameFindTarget(previousActiveMatch?.target, nextActiveMatch?.target);
  return {
    state: nextState,
    commands: nextState.activeMatchId === null
      ? [{ kind: 'clear-decorations' }]
      : activeMatchChanged || activeTargetChanged
        ? revealCommand(nextState)
        : [],
  };
}

export function nextFindSessionMatch<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed' || state.matches.length === 0) return { state, commands: [] };
  const currentIndex = indexOfActiveMatch(state);
  const nextIndex = currentIndex === -1 ? 0 : (currentIndex + 1) % state.matches.length;
  const nextState = { ...state, activeMatchId: state.matches[nextIndex]!.id };
  return { state: nextState, commands: revealCommand(nextState) };
}

export function previousFindSessionMatch<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed' || state.matches.length === 0) return { state, commands: [] };
  const currentIndex = indexOfActiveMatch(state);
  const previousIndex = currentIndex === -1
    ? state.matches.length - 1
    : (currentIndex - 1 + state.matches.length) % state.matches.length;
  const nextState = { ...state, activeMatchId: state.matches[previousIndex]!.id };
  return { state: nextState, commands: revealCommand(nextState) };
}

export function activateFindSessionMatch<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  matchId: MatchId,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return { state, commands: [] };
  if (!state.matches.some((match) => match.id === matchId)) return { state, commands: [] };
  const nextState = { ...state, activeMatchId: matchId };
  return { state: nextState, commands: revealCommand(nextState) };
}

export function resetFindSession<TTarget, TFocusOrigin>(): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  return {
    state: createClosedFindSession(),
    commands: [{ kind: 'clear-decorations' }],
  };
}

function replaceFindSessionSurface<TTarget, TFocusOrigin>(
  state: FindSessionState<TTarget, TFocusOrigin>,
  surface: SearchableSurface<TTarget, TFocusOrigin>,
): ReduceFindSessionResult<TTarget, TFocusOrigin> {
  if (state.status === 'closed') return openFindSession(state, surface);
  const nextState = createOpenState(surface, state.focusVersion + 1);
  return {
    state: nextState,
    commands: [
      { kind: 'clear-decorations' },
      { kind: 'focus-query', focusVersion: nextState.focusVersion },
      ...revealCommand(nextState),
    ],
  };
}

function createOpenState<TTarget, TFocusOrigin>(
  surface: SearchableSurface<TTarget, TFocusOrigin>,
  focusVersion: number,
): FindSessionOpenState<TTarget, TFocusOrigin> {
  return {
    status: 'open',
    surfaceKey: surface.key,
    query: surface.query,
    matches: surface.query.length === 0 ? [] : surface.matches,
    activeMatchId: surface.query.length === 0 ? null : surface.matches[0]?.id ?? null,
    focusOrigin: surface.focusOrigin,
    focusVersion,
  };
}

function reconcileOpenState<TTarget, TFocusOrigin>(
  state: FindSessionOpenState<TTarget, TFocusOrigin>,
  query: string,
  matches: readonly FindSessionMatch<TTarget>[],
  preferredMatchId: MatchId | null,
): FindSessionOpenState<TTarget, TFocusOrigin> {
  const nextMatches = query.length === 0 ? [] : matches;
  const nextActiveMatchId = query.length === 0
    ? null
    : reconcileActiveMatchId(state.matches, nextMatches, preferredMatchId);

  return {
    ...state,
    query,
    matches: nextMatches,
    activeMatchId: nextActiveMatchId,
  };
}

function reconcileActiveMatchId<TTarget>(
  previousMatches: readonly FindSessionMatch<TTarget>[],
  nextMatches: readonly FindSessionMatch<TTarget>[],
  preferredMatchId: MatchId | null,
): MatchId | null {
  if (nextMatches.length === 0) return null;
  if (preferredMatchId !== null && nextMatches.some((match) => match.id === preferredMatchId)) return preferredMatchId;
  if (preferredMatchId === null) return nextMatches[0]!.id;

  const previousIndex = previousMatches.findIndex((match) => match.id === preferredMatchId);
  if (previousIndex === -1) return nextMatches[0]!.id;

  let bestIndex = 0;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (let index = 0; index < nextMatches.length; index += 1) {
    const distance = Math.abs(index - previousIndex);
    if (distance < bestDistance) {
      bestDistance = distance;
      bestIndex = index;
    }
  }
  return nextMatches[bestIndex]!.id;
}

function indexOfActiveMatch<TTarget, TFocusOrigin>(state: FindSessionOpenState<TTarget, TFocusOrigin>): number {
  return state.activeMatchId === null ? -1 : state.matches.findIndex((match) => match.id === state.activeMatchId);
}

function sameFindTarget(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (typeof left !== typeof right || left === null || right === null) return false;
  if (typeof left !== 'object') return false;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left)
      && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => sameFindTarget(value, right[index]));
  }
  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord);
  const rightKeys = Object.keys(rightRecord);
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key) => Object.hasOwn(rightRecord, key) && sameFindTarget(leftRecord[key], rightRecord[key]));
}

function revealCommand<TTarget, TFocusOrigin>(
  state: FindSessionOpenState<TTarget, TFocusOrigin>,
): readonly FindSessionCommand<TTarget, TFocusOrigin>[] {
  if (state.activeMatchId === null) return [];
  const match = state.matches.find((candidate) => candidate.id === state.activeMatchId);
  return match ? [{ kind: 'reveal-match', matchId: match.id, target: match.target }] : [];
}
