import { useEffect, useRef, useState } from 'react';
import type { CredentialStatus } from '../api';

/**
 * Manages auto-opening the credential helper panel when credential status
 * needs attention. Consolidates the duplicated pattern from ConversationPage
 * and ConversationListPage.
 */
export function useAutoAuth(credentialStatus: CredentialStatus | null) {
  const [showAuthPanel, setShowAuthPanel] = useState(false);
  const autoAuthAttemptedRef = useRef(false);

  useEffect(() => {
    if (
      (credentialStatus === 'required' ||
        credentialStatus === 'running' ||
        credentialStatus === 'failed') &&
      !autoAuthAttemptedRef.current &&
      !showAuthPanel
    ) {
      autoAuthAttemptedRef.current = true;
      setShowAuthPanel(true);
    }
  }, [credentialStatus, showAuthPanel]);

  return { showAuthPanel, setShowAuthPanel };
}
