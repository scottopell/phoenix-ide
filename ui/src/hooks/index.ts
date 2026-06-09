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
export { useConversationPrStatus } from './useConversationPrStatus';
export type { ConversationPrStatusHandle, ConversationPrStatusState } from './useConversationPrStatus';
export { useTheme } from './useTheme';
export { useDensity, isSignificantText, SIGNIFICANCE_THRESHOLD } from './useDensity';
export type { Density } from './useDensity';
export { useScopedState } from './useScopedState';
export { useInlineReferences } from './useInlineReferences';
export type { UseInlineReferencesParams, InlineReferences } from './useInlineReferences';
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
