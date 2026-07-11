export interface ViewerFindState {
  isOpen: boolean;
  query: string;
  activeIndex: number;
}

export type ViewerFindAction =
  | { type: 'open' }
  | { type: 'close' }
  | { type: 'toggle' }
  | { type: 'set-query'; query: string }
  | { type: 'set-active-index'; index: number }
  | { type: 'next-match'; matchCount: number }
  | { type: 'previous-match'; matchCount: number };

export const initialViewerFindState: ViewerFindState = {
  isOpen: false,
  query: '',
  activeIndex: -1,
};

function normalizeActiveIndex(index: number, matchCount: number): number {
  if (matchCount <= 0) return -1;
  if (index < 0) return 0;
  if (index >= matchCount) return matchCount - 1;
  return index;
}

function wrapIndex(index: number, matchCount: number): number {
  if (matchCount <= 0) return -1;
  const wrapped = index % matchCount;
  return wrapped >= 0 ? wrapped : wrapped + matchCount;
}

export function viewerFindReducer(state: ViewerFindState, action: ViewerFindAction): ViewerFindState {
  switch (action.type) {
    case 'open':
      return state.isOpen ? state : { ...state, isOpen: true };
    case 'close':
      return state.isOpen ? { ...state, isOpen: false } : state;
    case 'toggle':
      return { ...state, isOpen: !state.isOpen };
    case 'set-query':
      return {
        ...state,
        isOpen: true,
        query: action.query,
        activeIndex: action.query.length > 0 ? 0 : -1,
      };
    case 'set-active-index':
      return { ...state, activeIndex: action.index };
    case 'next-match': {
      const normalized = normalizeActiveIndex(state.activeIndex, action.matchCount);
      return { ...state, activeIndex: wrapIndex(normalized + 1, action.matchCount) };
    }
    case 'previous-match': {
      const normalized = normalizeActiveIndex(state.activeIndex, action.matchCount);
      return {
        ...state,
        activeIndex: normalized === -1
          ? normalizeActiveIndex(action.matchCount - 1, action.matchCount)
          : wrapIndex(normalized - 1, action.matchCount),
      };
    }
    default:
      return state;
  }
}
