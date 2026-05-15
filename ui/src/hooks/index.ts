export { useLocalStorage, useLocalStorageString } from './useLocalStorage';
export { useKeyboardNav, useGlobalKeyboardShortcuts } from './useKeyboardNav';
export {
  useDraftValue,
  useDraftActions,
  useDraftLifecycle,
  DraftLifecycle,
} from './useDraft';
export type { DraftActions } from './useDraft';
export { FocusScopeProvider, useFocusScope, useRegisterFocusScope } from './useFocusScope';
export {
  useMessageQueue,
  derivePendingMessages,
  deriveFailedMessages,
} from './useMessageQueue';
export type { QueuedMessage, MessageStatus } from './useMessageQueue';
export { useConnection } from './useConnection';
export type { ConnectionState, ConnectionInfo } from './useConnection';
export { useResizablePane } from './useResizablePane';
export type { UseResizablePaneOptions, UseResizablePaneResult } from './useResizablePane';
export { useModels } from './useModels';
export { useAutoAuth } from './useAutoAuth';
export { useTheme } from './useTheme';
export { useScopedState } from './useScopedState';
export { useMediaQuery, useIsDesktop, useIsWideDesktop, useIsMobile } from './useMediaQuery';

// Export state machine for testing
export {
  transition,
  initialState,
  checkInvariants,
  getBackoffDelay,
  BACKOFF_BASE_MS,
  BACKOFF_MAX_MS,
  OFFLINE_THRESHOLD,
  RECONNECTED_DISPLAY_MS,
} from './connectionMachine';
export type {
  ConnectionMachineState,
  ConnectionInput,
  ConnectionEffect,
  ConnectionTransitionResult,
  TransitionContext,
} from './connectionMachine';
