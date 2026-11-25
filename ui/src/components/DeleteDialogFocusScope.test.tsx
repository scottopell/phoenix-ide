import { useEffect } from 'react';
import { render, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { FocusScopeProvider, useFocusScope } from '../hooks/useFocusScope';
import { ConfirmDialog } from './ConfirmDialog';

function ScopeCapture({ onScope }: { onScope: (scope: string | null) => void }) {
  const { activeScope } = useFocusScope();
  useEffect(() => onScope(activeScope), [activeScope, onScope]);
  return null;
}

describe('persistent delete dialog focus scopes', () => {
  it('registers a visible conversation confirm dialog', async () => {
    let activeScope: string | null = null;
    render(
      <FocusScopeProvider>
        <ScopeCapture onScope={(scope) => { activeScope = scope; }} />
        <ConfirmDialog
          visible
          title="Delete conversation"
          message="This cannot be undone"
          onConfirm={vi.fn()}
          onCancel={vi.fn()}
        />
      </FocusScopeProvider>,
    );
    await waitFor(() => expect(activeScope).toBe('confirm-dialog'));
  });
});