import { useEffect } from 'react';
import { render, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ChainView } from '../api';
import { FocusScopeProvider, useFocusScope } from '../hooks/useFocusScope';
import { ChainDeleteConfirm } from './ChainDeleteConfirm';
import { ConfirmDialog } from './ConfirmDialog';

function ScopeCapture({ onScope }: { onScope: (scope: string | null) => void }) {
  const { activeScope } = useFocusScope();
  useEffect(() => onScope(activeScope), [activeScope, onScope]);
  return null;
}

const chain: ChainView = {
  root_conv_id: 'root',
  chain_name: 'chain',
  display_name: 'chain',
  archived: false,
  members: [],
  qa_history: [],
  current_member_count: 0,
  current_total_messages: 0,
  work_identity: null,
};

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

  it('registers a visible chain delete dialog', async () => {
    let activeScope: string | null = null;
    render(
      <FocusScopeProvider>
        <ScopeCapture onScope={(scope) => { activeScope = scope; }} />
        <ChainDeleteConfirm
          visible
          chain={chain}
          onConfirm={vi.fn()}
          onCancel={vi.fn()}
        />
      </FocusScopeProvider>,
    );
    await waitFor(() => expect(activeScope).toBe('chain-delete-confirm'));
  });
});
