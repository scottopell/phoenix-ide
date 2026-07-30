export const WAKE_STATUS_CHANGED_EVENT = 'phoenix:wake-status-changed';

export function notifyWakeStatusChanged(conversationId: string | undefined): void {
  if (!conversationId) return;
  window.dispatchEvent(
    new CustomEvent(WAKE_STATUS_CHANGED_EVENT, {
      detail: { conversationId },
    }),
  );
}
