export const PIN_TO_BOTTOM_THRESHOLD = 100;
export const SETTLE_WATCH_MS = 3000;
export const SETTLE_WATCH_INTERVAL_MS = 150;

export interface ScrollSnapshot {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

export type TailActivity = 'none' | 'active';

export type FollowMode =
  | { kind: 'following' }
  | { kind: 'reading' }
  | { kind: 'navigating'; departedBottom: boolean }
  | { kind: 'returning-to-tail' };

export type Gesture =
  | { kind: 'idle' }
  | {
      kind: 'touch';
      moved: boolean;
      departedBottom: boolean;
      modeBeforeGesture: FollowMode;
    };

export interface ScrollGeometry {
  atBottom: boolean;
  lastSnapshot: ScrollSnapshot | null;
  previousTotalHeight: number;
  previousScrollHeight: number;
  previousClientHeight: number;
}

interface MeasuredSession {
  conversationId: string | undefined;
  geometry: ScrollGeometry;
  gesture: Gesture;
  unread: boolean;
}

type UnreadySession =
  | { kind: 'unmeasured'; conversationId: string | undefined }
  | { kind: 'measured-empty'; conversationId: string | undefined };

type ReadySession =
  | (MeasuredSession & { kind: 'mount-rescue'; deadlineMs: number })
  | (MeasuredSession & { kind: 'live'; follow: FollowMode });

export type ScrollMachineState = UnreadySession | ReadySession;

export type ScrollEffect =
  | { type: 'snapToLastIndex' }
  | { type: 'scheduleTailFollow'; conversationId: string | undefined }
  | { type: 'scheduleDomBottomWrite' }
  | { type: 'writeDomBottom' }
  | { type: 'startSettleWatch'; deadlineMs: number }
  | { type: 'stopSettleWatch' }
  | { type: 'showUnread' }
  | { type: 'clearUnread' };

export type ScrollEvent =
  | { type: 'conversationChanged'; conversationId: string | undefined }
  | {
      type: 'conversationMeasured';
      conversationId: string | undefined;
      totalHeight: number;
      unitCount: number;
      snapshot: ScrollSnapshot | null;
      nowMs: number;
    }
  | { type: 'scrollerAttached'; snapshot: ScrollSnapshot }
  | { type: 'viewportPinnedChanged'; atBottom: boolean }
  | { type: 'interactionStarted' }
  | { type: 'touchStarted' }
  | { type: 'touchMoved' }
  | { type: 'touchEnded'; remainingTouches: number }
  | { type: 'touchCancelled'; remainingTouches: number }
  | { type: 'upwardIntent'; snapshot?: ScrollSnapshot }
  | { type: 'downwardMovement'; snapshot: ScrollSnapshot }
  | { type: 'navigationJumped' }
  | { type: 'tailContentAdvanced' }
  | {
      type: 'heightChanged';
      totalHeight: number;
      unitCount: number;
      snapshot: ScrollSnapshot | null;
      tailActivity: TailActivity;
    }
  | { type: 'settleProbe'; snapshot: ScrollSnapshot | null; nowMs: number }
  | { type: 'jumpToNewestRequested'; unitCount: number };

interface Reduction {
  state: ScrollMachineState;
  effects: ScrollEffect[];
}

const IDLE: Gesture = { kind: 'idle' };
const FOLLOWING: FollowMode = { kind: 'following' };
const READING: FollowMode = { kind: 'reading' };
const RETURNING: FollowMode = { kind: 'returning-to-tail' };

function navigationMode(state: ReadySession): FollowMode {
  return { kind: 'navigating', departedBottom: !state.geometry.atBottom };
}

function isReady(state: ScrollMachineState): state is ReadySession {
  return state.kind === 'mount-rescue' || state.kind === 'live';
}

function geometryFrom(snapshot: ScrollSnapshot | null, totalHeight: number): ScrollGeometry {
  return {
    atBottom: true,
    lastSnapshot: snapshot,
    previousTotalHeight: totalHeight,
    previousScrollHeight: snapshot?.scrollHeight ?? 0,
    previousClientHeight: snapshot?.clientHeight ?? 0,
  };
}

function updateGeometry(
  geometry: ScrollGeometry,
  snapshot: ScrollSnapshot | null,
  totalHeight = geometry.previousTotalHeight,
): ScrollGeometry {
  return {
    ...geometry,
    lastSnapshot: snapshot ?? geometry.lastSnapshot,
    previousTotalHeight: totalHeight,
    previousScrollHeight: snapshot?.scrollHeight ?? geometry.previousScrollHeight,
    previousClientHeight: snapshot?.clientHeight ?? geometry.previousClientHeight,
  };
}

export function initialScrollMachineState(
  conversationId?: string,
): ScrollMachineState {
  return { kind: 'unmeasured', conversationId };
}

function unreadEffects(wasUnread: boolean, unread: boolean): ScrollEffect[] {
  if (wasUnread === unread) return [];
  return [{ type: unread ? 'showUnread' : 'clearUnread' }];
}

function liveFrom(
  session: MeasuredSession,
  follow: FollowMode,
): ScrollMachineState {
  return { ...session, kind: 'live', follow };
}

function exitMountRescue(
  state: Extract<ScrollMachineState, { kind: 'mount-rescue' }>,
  follow: FollowMode,
): Reduction {
  return {
    state: liveFrom(
      {
        conversationId: state.conversationId,
        geometry: state.geometry,
        gesture: state.gesture,
        unread: state.unread,
      },
      follow,
    ),
    effects: [{ type: 'stopSettleWatch' }],
  };
}

function takeUserOwnership(
  state: ReadySession,
  snapshot?: ScrollSnapshot,
): Reduction {
  const session: MeasuredSession = {
    conversationId: state.conversationId,
    geometry: snapshot ? updateGeometry(state.geometry, snapshot) : state.geometry,
    gesture: state.gesture,
    unread: state.unread,
  };
  return {
    state: liveFrom(session, READING),
    effects: state.kind === 'mount-rescue' ? [{ type: 'stopSettleWatch' }] : [],
  };
}

function requestTailReturn(
  state: Extract<ScrollMachineState, { kind: 'live' }>,
): Reduction {
  const effects = unreadEffects(state.unread, false);
  return {
    state: { ...state, follow: RETURNING, unread: false },
    effects: [...effects, { type: 'snapToLastIndex' }],
  };
}

function confirmTailReturn(
  state: ReadySession,
): Reduction {
  const effects = unreadEffects(state.unread, false);
  const session: MeasuredSession = {
    conversationId: state.conversationId,
    geometry: { ...state.geometry, atBottom: true },
    gesture: state.gesture,
    unread: false,
  };
  return {
    state: liveFrom(session, FOLLOWING),
    effects: state.kind === 'mount-rescue'
      ? [{ type: 'stopSettleWatch' }, ...effects]
      : effects,
  };
}

function advanceTail(
  state: ReadySession,
): Reduction {
  const blocked =
    (state.kind === 'live' && (state.follow.kind === 'reading' || state.follow.kind === 'navigating')) ||
    (state.gesture.kind === 'touch' && state.gesture.moved);
  const unread = blocked;
  if (state.kind === 'mount-rescue') {
    return {
      state: { ...state, unread },
      effects: [
        ...unreadEffects(state.unread, unread),
        { type: 'scheduleDomBottomWrite' },
      ],
    };
  }
  return {
    state: { ...state, unread },
    effects: blocked
      ? unreadEffects(state.unread, unread)
      : [
          ...unreadEffects(state.unread, unread),
          { type: 'scheduleTailFollow', conversationId: state.conversationId },
        ],
  };
}

function resolveTouch(
  state: ReadySession,
  remainingTouches: number,
): Reduction {
  if (state.gesture.kind !== 'touch' || remainingTouches > 0) {
    return { state, effects: [] };
  }
  const follow = state.gesture.moved ? READING : state.gesture.modeBeforeGesture;
  const session: MeasuredSession = {
    conversationId: state.conversationId,
    geometry: state.geometry,
    gesture: IDLE,
    unread: state.unread,
  };
  return { state: liveFrom(session, follow), effects: [] };
}

export function reduceScrollMachine(
  state: ScrollMachineState,
  event: ScrollEvent,
): Reduction {
  switch (event.type) {
    case 'conversationChanged': {
      if (state.conversationId === event.conversationId) return { state, effects: [] };
      const effects: ScrollEffect[] = [];
      if (state.kind === 'mount-rescue') effects.push({ type: 'stopSettleWatch' });
      if ((state.kind === 'mount-rescue' || state.kind === 'live') && state.unread) {
        effects.push({ type: 'clearUnread' });
      }
      return {
        state: initialScrollMachineState(event.conversationId),
        effects,
      };
    }

    case 'conversationMeasured': {
      if (state.conversationId !== event.conversationId) {
        const reset = reduceScrollMachine(state, {
          type: 'conversationChanged',
          conversationId: event.conversationId,
        });
        const measured = reduceScrollMachine(reset.state, event);
        return {
          state: measured.state,
          effects: [...reset.effects, ...measured.effects],
        };
      }
      if (state.kind === 'mount-rescue' || state.kind === 'live') {
        return reduceScrollMachine(state, {
          type: 'heightChanged',
          totalHeight: event.totalHeight,
          unitCount: event.unitCount,
          snapshot: event.snapshot,
          tailActivity: 'none',
        });
      }
      if (event.unitCount === 0 || event.snapshot === null) {
        return {
          state: event.unitCount === 0
            ? { kind: 'measured-empty', conversationId: event.conversationId }
            : state,
          effects: [],
        };
      }
      const firstContentAfterEmpty = state.kind === 'measured-empty';
      const deadlineMs = event.nowMs + SETTLE_WATCH_MS;
      return {
        state: {
          kind: 'mount-rescue',
          conversationId: event.conversationId,
          deadlineMs,
          geometry: geometryFrom(event.snapshot, event.totalHeight),
          gesture: IDLE,
          unread: false,
        },
        effects: [
          ...(firstContentAfterEmpty ? [{ type: 'snapToLastIndex' } as const] : []),
          { type: 'startSettleWatch', deadlineMs },
          { type: 'scheduleDomBottomWrite' },
        ],
      };
    }

    case 'scrollerAttached':
      if (!isReady(state)) return { state, effects: [] };
      return {
        state: { ...state, geometry: updateGeometry(state.geometry, event.snapshot) },
        effects: [],
      };

    case 'viewportPinnedChanged': {
      if (!isReady(state)) return { state, effects: [] };
      const geometry = { ...state.geometry, atBottom: event.atBottom };
      const gesture = state.gesture.kind === 'touch' && !event.atBottom
        ? { ...state.gesture, departedBottom: true }
        : state.gesture;
      const next: ReadySession = state.kind === 'live'
        ? {
            ...state,
            geometry,
            gesture,
            follow: state.follow.kind === 'navigating' && !event.atBottom
              ? { ...state.follow, departedBottom: true }
              : state.follow,
          }
        : { ...state, geometry, gesture };
      if (!event.atBottom || next.kind === 'mount-rescue') {
        return { state: next, effects: [] };
      }
      if (
        (next.kind === 'live' && next.follow.kind === 'navigating' && !next.follow.departedBottom) ||
        (next.gesture.kind === 'touch' && next.gesture.moved)
      ) {
        return { state: next, effects: [] };
      }
      return confirmTailReturn(next);
    }

    case 'interactionStarted':
      if (state.kind === 'mount-rescue') return exitMountRescue(state, FOLLOWING);
      return { state, effects: [] };

    case 'touchStarted': {
      if (!isReady(state)) return { state, effects: [] };
      const follow = state.kind === 'live' ? state.follow : FOLLOWING;
      const session: MeasuredSession = {
        conversationId: state.conversationId,
        geometry: state.geometry,
        gesture: {
          kind: 'touch',
          moved: false,
          departedBottom: !state.geometry.atBottom,
          modeBeforeGesture: follow,
        },
        unread: state.unread,
      };
      return {
        state: liveFrom(session, follow),
        effects: state.kind === 'mount-rescue' ? [{ type: 'stopSettleWatch' }] : [],
      };
    }

    case 'touchMoved': {
      if (!isReady(state) || state.gesture.kind !== 'touch') {
        return { state, effects: [] };
      }
      return {
        state: {
          ...state,
          kind: 'live',
          follow: READING,
          gesture: { ...state.gesture, moved: true },
        },
        effects: state.kind === 'mount-rescue' ? [{ type: 'stopSettleWatch' }] : [],
      };
    }

    case 'touchEnded':
    case 'touchCancelled':
      if (!isReady(state)) return { state, effects: [] };
      return resolveTouch(state, event.remainingTouches);

    case 'upwardIntent':
      if (!isReady(state)) return { state, effects: [] };
      if (state.kind === 'live' && state.follow.kind === 'navigating') {
        return {
          state: {
            ...state,
            geometry: event.snapshot ? updateGeometry(state.geometry, event.snapshot) : state.geometry,
            follow: { ...state.follow, departedBottom: true },
          },
          effects: [],
        };
      }
      return takeUserOwnership(state, event.snapshot);

    case 'navigationJumped':
      if (!isReady(state)) return { state, effects: [] };
      if (state.kind === 'mount-rescue') return exitMountRescue(state, navigationMode(state));
      return { state: { ...state, follow: navigationMode(state) }, effects: [] };

    case 'downwardMovement':
      if (!isReady(state)) return { state, effects: [] };
      return {
        state: { ...state, geometry: updateGeometry(state.geometry, event.snapshot) },
        effects: [],
      };

    case 'tailContentAdvanced':
      if (!isReady(state)) return { state, effects: [] };
      return advanceTail(state);

    case 'heightChanged': {
      if (!isReady(state)) return { state, effects: [] };
      const previousTotalHeight = state.geometry.previousTotalHeight;
      const nextState = {
        ...state,
        geometry: updateGeometry(state.geometry, event.snapshot, event.totalHeight),
      };
      if (event.unitCount === 0 || event.snapshot === null) {
        return { state: nextState, effects: [] };
      }
      if (state.kind === 'mount-rescue') {
        return { state: nextState, effects: [{ type: 'scheduleDomBottomWrite' }] };
      }
      if (
        state.follow.kind !== 'reading' &&
        state.follow.kind !== 'navigating' &&
        !(state.gesture.kind === 'touch' && state.gesture.moved)
      ) {
        return {
          state: { ...nextState, unread: false },
          effects: [
            ...unreadEffects(state.unread, false),
            { type: 'snapToLastIndex' },
          ],
        };
      }
      const tailGrew =
        event.tailActivity === 'active' && event.totalHeight > previousTotalHeight;
      if (!tailGrew || state.unread) return { state: nextState, effects: [] };
      return {
        state: { ...nextState, unread: true },
        effects: [{ type: 'showUnread' }],
      };
    }

    case 'settleProbe': {
      if (state.kind !== 'mount-rescue') return { state, effects: [] };
      if (event.nowMs > state.deadlineMs) return exitMountRescue(state, FOLLOWING);
      if (
        event.snapshot &&
        event.snapshot.scrollHeight - event.snapshot.scrollTop - event.snapshot.clientHeight > 1
      ) {
        return {
          state: {
            ...state,
            geometry: updateGeometry(state.geometry, event.snapshot),
          },
          effects: [{ type: 'writeDomBottom' }],
        };
      }
      return { state, effects: [] };
    }

    case 'jumpToNewestRequested':
      if (!isReady(state) || event.unitCount === 0) {
        return { state, effects: [] };
      }
      if (state.kind === 'mount-rescue') {
        const exited = exitMountRescue(state, FOLLOWING);
        const live = exited.state as Extract<ScrollMachineState, { kind: 'live' }>;
        const requested = requestTailReturn(live);
        return { state: requested.state, effects: [...exited.effects, ...requested.effects] };
      }
      return requestTailReturn(state);
  }
}
