import { useCallback, useState } from 'react';

export function useFocusedReviewExit({
  noteCount,
  send,
  discard,
  returnToPane,
}: {
  noteCount: number;
  send: () => Promise<void>;
  discard: () => void;
  returnToPane: () => void;
}) {
  const [promptOpen, setPromptOpen] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const requestReturn = useCallback(() => {
    setError(null);
    if (noteCount > 0) {
      setPromptOpen(true);
      return;
    }
    returnToPane();
  }, [noteCount, returnToPane]);

  const keepReviewing = useCallback(() => {
    if (sending) return;
    setPromptOpen(false);
    setError(null);
  }, [sending]);

  const discardAndReturn = useCallback(() => {
    discard();
    setPromptOpen(false);
    setError(null);
    returnToPane();
  }, [discard, returnToPane]);

  const sendAndReturn = useCallback(async () => {
    if (sending) return;
    setSending(true);
    setError(null);
    try {
      await send();
      setPromptOpen(false);
      returnToPane();
    } catch (cause) {
      setPromptOpen(true);
      setError(cause instanceof Error ? cause.message : 'Feedback could not be sent. Try again.');
    } finally {
      setSending(false);
    }
  }, [returnToPane, send, sending]);

  return {
    promptOpen,
    sending,
    error,
    requestReturn,
    keepReviewing,
    discardAndReturn,
    sendAndReturn,
  };
}
