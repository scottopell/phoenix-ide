import { useCallback, useState } from 'react';

export type FocusedReviewExitTarget = 'pane' | 'closed';

export function useFocusedReviewExit({
  noteCount,
  send,
  discard,
  returnToPane,
  closeViewer,
}: {
  noteCount: number;
  send: () => Promise<void>;
  discard: () => void;
  returnToPane: () => void;
  closeViewer: () => void;
}) {
  const [exitTarget, setExitTarget] = useState<FocusedReviewExitTarget | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const requestExit = useCallback((target: FocusedReviewExitTarget) => {
    setError(null);
    if (noteCount > 0) {
      setExitTarget(target);
      return;
    }
    if (target === 'pane') returnToPane();
    else closeViewer();
  }, [closeViewer, noteCount, returnToPane]);

  const requestReturn = useCallback(() => requestExit('pane'), [requestExit]);
  const requestClose = useCallback(() => requestExit('closed'), [requestExit]);

  const keepReviewing = useCallback(() => {
    if (sending) return;
    setExitTarget(null);
    setError(null);
  }, [sending]);

  const discardAndReturn = useCallback(() => {
    if (sending) return;
    discard();
    setExitTarget(null);
    setError(null);
    if (exitTarget === 'closed') closeViewer();
    else returnToPane();
  }, [closeViewer, discard, exitTarget, returnToPane, sending]);

  const sendAndReturn = useCallback(async () => {
    if (sending) return;
    setSending(true);
    setError(null);
    try {
      await send();
      setExitTarget(null);
      if (exitTarget === 'closed') closeViewer();
      else returnToPane();
    } catch (cause) {
      setExitTarget(exitTarget ?? 'pane');
      setError(cause instanceof Error ? cause.message : 'Feedback could not be sent. Try again.');
    } finally {
      setSending(false);
    }
  }, [closeViewer, exitTarget, returnToPane, send, sending]);

  return {
    exitTarget,
    sending,
    error,
    requestReturn,
    requestClose,
    keepReviewing,
    discardAndReturn,
    sendAndReturn,
  };
}
