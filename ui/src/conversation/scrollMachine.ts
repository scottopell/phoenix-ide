export const PIN_TO_BOTTOM_THRESHOLD = 100;
export const USER_SCROLL_SUPPRESS_MS = 400;
export const TOUCH_GESTURE_SUPPRESS_MS = 1200;
export const SETTLE_WATCH_MS = 3000;
export const SETTLE_WATCH_INTERVAL_MS = 150;

export interface ScrollSnapshot {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

export type TailActivity = 'none' | 'active';

export type ScrollEffect =
  | { type: 'snapToLastIndex' }
  | { type: 'scheduleDomBottomWrite' }
  | { type: 'writeDomBottom' }
  | { type: 'startSettleWatch'; deadlineMs: number }
  | { type: 'stopSettleWatch' }
  | { type: 'showUnread' }
  | { type: 'clearUnread' }
  | { type: 'debugIgnoredGrowth'; oldFromBottom: number; heightDelta: number };

export interface ScrollMachineState {
  conversationId: string | undefined;
  hasMeasuredConversation: boolean;
  prevTotalHeight: number;
  prevScrollHeight: number;
  prevClientHeight: number;
  hasSeenContent: boolean;
  hasUserEngaged: boolean;
  touchActive: boolean;
  lastUpwardScrollAt: number;
  lastTouchGestureAt: number;
  lastScrollTop: number;
  settleDeadline: number;
}

export type ScrollEvent =
  | { type: 'scrollerAttached'; snapshot: ScrollSnapshot }
  | { type: 'atBottomChanged'; atBottom: boolean }
  | { type: 'pointerDown' }
  | { type: 'touchStart'; nowMs: number }
  | { type: 'touchEnd'; remainingTouches: number; nowMs: number }
  | { type: 'wheel'; deltaY: number; nowMs: number }
  | { type: 'scroll'; snapshot: ScrollSnapshot; nowMs: number }
  | { type: 'totalHeightChanged'; conversationId: string | undefined; totalHeight: number; unitCount: number; snapshot: ScrollSnapshot | null; tailActivity: TailActivity; nowMs: number }
  | { type: 'settleTick'; snapshot: ScrollSnapshot | null; nowMs: number }
  | { type: 'navJump' }
  | { type: 'jumpToNewestClicked'; unitCount: number };

export function initialScrollMachineState(): ScrollMachineState {
  return {
    conversationId: undefined,
    hasMeasuredConversation: false,
    prevTotalHeight: 0,
    prevScrollHeight: 0,
    prevClientHeight: 0,
    hasSeenContent: false,
    hasUserEngaged: false,
    touchActive: false,
    lastUpwardScrollAt: 0,
    lastTouchGestureAt: 0,
    lastScrollTop: 0,
    settleDeadline: 0,
  };
}

function touchGestureTimestamp(state: ScrollMachineState, nowMs: number): number {
  return state.lastUpwardScrollAt > 0 && nowMs - state.lastUpwardScrollAt < TOUCH_GESTURE_SUPPRESS_MS
    ? nowMs
    : state.lastTouchGestureAt;
}

export function reduceScrollMachine(
  state: ScrollMachineState,
  event: ScrollEvent,
): { state: ScrollMachineState; effects: ScrollEffect[] } {
  switch (event.type) {
    case 'scrollerAttached':
      return {
        state: {
          ...state,
          touchActive: false,
          lastUpwardScrollAt: 0,
          lastTouchGestureAt: 0,
          lastScrollTop: event.snapshot.scrollTop,
          hasUserEngaged: false,
        },
        effects: [],
      };
    case 'atBottomChanged':
      return { state, effects: event.atBottom ? [{ type: 'clearUnread' }] : [] };
    case 'pointerDown':
    case 'navJump':
      return { state: { ...state, hasUserEngaged: true }, effects: [] };
    case 'touchStart':
      return {
        state: {
          ...state,
          hasUserEngaged: true,
          touchActive: true,
          lastTouchGestureAt: touchGestureTimestamp(state, event.nowMs),
        },
        effects: [],
      };
    case 'touchEnd':
      return {
        state: {
          ...state,
          touchActive: event.remainingTouches > 0,
          lastTouchGestureAt: touchGestureTimestamp(state, event.nowMs),
        },
        effects: [],
      };
    case 'wheel':
      return {
        state: {
          ...state,
          hasUserEngaged: true,
          lastUpwardScrollAt: event.deltaY < 0 ? event.nowMs : state.lastUpwardScrollAt,
        },
        effects: [],
      };
    case 'scroll': {
      const upward = event.snapshot.scrollTop < state.lastScrollTop;
      const nextState = {
        ...state,
        lastScrollTop: event.snapshot.scrollTop,
        lastUpwardScrollAt: upward ? event.nowMs : state.lastUpwardScrollAt,
      };
      if (
        !nextState.hasUserEngaged &&
        event.nowMs <= nextState.settleDeadline &&
        event.snapshot.scrollHeight - event.snapshot.scrollTop - event.snapshot.clientHeight > 1
      ) {
        return { state: nextState, effects: [{ type: 'scheduleDomBottomWrite' }] };
      }
      return { state: nextState, effects: [] };
    }
    case 'totalHeightChanged': {
      if (!state.hasMeasuredConversation || state.conversationId !== event.conversationId) {
        const hasSeenContent = event.unitCount > 0;
        return {
          state: {
            ...state,
            conversationId: event.conversationId,
            hasMeasuredConversation: true,
            prevTotalHeight: event.totalHeight,
            prevClientHeight: event.snapshot?.clientHeight ?? 0,
            prevScrollHeight: event.snapshot?.scrollHeight ?? 0,
            hasSeenContent,
            hasUserEngaged: false,
            touchActive: false,
            lastUpwardScrollAt: 0,
            lastTouchGestureAt: 0,
            lastScrollTop: event.snapshot?.scrollTop ?? 0,
            settleDeadline: hasSeenContent ? event.nowMs + SETTLE_WATCH_MS : state.settleDeadline,
          },
          effects: hasSeenContent
            ? [{ type: 'startSettleWatch', deadlineMs: event.nowMs + SETTLE_WATCH_MS }, { type: 'scheduleDomBottomWrite' }]
            : [],
        };
      }

      const prevHeight = state.prevTotalHeight;
      let nextState = { ...state, prevTotalHeight: event.totalHeight };
      if (event.unitCount === 0 || event.snapshot === null) return { state: nextState, effects: [] };

      if (!nextState.hasSeenContent) {
        nextState = {
          ...nextState,
          hasSeenContent: true,
          prevClientHeight: event.snapshot.clientHeight,
          prevScrollHeight: event.snapshot.scrollHeight,
          settleDeadline: event.nowMs + SETTLE_WATCH_MS,
        };
        return {
          state: nextState,
          effects: [
            { type: 'snapToLastIndex' },
            { type: 'startSettleWatch', deadlineMs: event.nowMs + SETTLE_WATCH_MS },
            { type: 'scheduleDomBottomWrite' },
          ],
        };
      }

      const clientHeightForPinCheck =
        event.snapshot.clientHeight < nextState.prevClientHeight
          ? nextState.prevClientHeight
          : event.snapshot.clientHeight;
      const oldFromBottom = nextState.prevScrollHeight - event.snapshot.scrollTop - clientHeightForPinCheck;
      nextState = {
        ...nextState,
        prevClientHeight: event.snapshot.clientHeight,
        prevScrollHeight: event.snapshot.scrollHeight,
      };

      if (!nextState.hasUserEngaged && event.nowMs <= nextState.settleDeadline) {
        return { state: nextState, effects: [{ type: 'scheduleDomBottomWrite' }] };
      }

      const grewWithTailActivity = event.totalHeight > prevHeight && event.tailActivity === 'active';
      if (oldFromBottom <= PIN_TO_BOTTOM_THRESHOLD) {
        const userOwnsViewport =
          nextState.touchActive ||
          event.nowMs - nextState.lastUpwardScrollAt < USER_SCROLL_SUPPRESS_MS ||
          (nextState.lastTouchGestureAt > 0 && event.nowMs - nextState.lastTouchGestureAt < TOUCH_GESTURE_SUPPRESS_MS);
        if (!userOwnsViewport) return { state: nextState, effects: [{ type: 'snapToLastIndex' }] };
        if (grewWithTailActivity) return { state: nextState, effects: [{ type: 'showUnread' }] };
        return { state: nextState, effects: [] };
      }

      if (event.totalHeight > prevHeight) {
        if (grewWithTailActivity) return { state: nextState, effects: [{ type: 'showUnread' }] };
        return {
          state: nextState,
          effects: [{ type: 'debugIgnoredGrowth', oldFromBottom, heightDelta: event.totalHeight - prevHeight }],
        };
      }
      return { state: nextState, effects: [] };
    }
    case 'settleTick':
      if (state.hasUserEngaged || event.nowMs > state.settleDeadline) {
        return { state, effects: [{ type: 'stopSettleWatch' }] };
      }
      if (
        event.snapshot &&
        event.snapshot.scrollHeight - event.snapshot.scrollTop - event.snapshot.clientHeight > 1
      ) {
        return { state, effects: [{ type: 'writeDomBottom' }] };
      }
      return { state, effects: [] };
    case 'jumpToNewestClicked':
      return {
        state,
        effects: event.unitCount === 0 ? [] : [{ type: 'clearUnread' }, { type: 'snapToLastIndex' }],
      };
  }
}
