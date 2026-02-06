export { useLocalStorage, useLocalStorageString } from './useLocalStorage';
export { useDraft } from './useDraft';
export { useMessageQueue } from './useMessageQueue';
export type { QueuedMessage, MessageStatus } from './useMessageQueue';
export { useConnection } from './useConnection';
export type { ConnectionState, ConnectionInfo } from './useConnection';

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
