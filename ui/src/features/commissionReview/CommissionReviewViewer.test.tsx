import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { Message } from '../../api';
import { FocusScopeProvider, useFocusScope } from '../../hooks/useFocusScope';
import { CommissionReviewViewer } from './CommissionReviewViewer';

function ActiveScope() {
  const { activeScope } = useFocusScope();
  return <output data-testid="active-focus-scope">{activeScope ?? 'none'}</output>;
}

const request: Message = {
  message_id: 'request',
  sequence_id: 7,
  conversation_id: 'conv',
  message_type: 'agent',
  content: [{
    type: 'tool_use',
    id: 'review-tool',
    name: 'commission_review',
    input: { brief: 'Review the change' },
  }],
  created_at: '2026-01-01T00:00:00Z',
};

describe('CommissionReviewViewer', () => {
  it('keeps the same scroll surface across pane and fullscreen presentation', () => {
    const props = {
      sequenceId: 7,
      messages: [request],
      onClose: () => {},
      canTogglePresentation: true,
      onPresentationChange: () => {},
      inline: true,
    };
    const view = render(<CommissionReviewViewer {...props} presentation="pane" />);
    const paneContent = view.container.querySelector('.viewer-content') as HTMLDivElement;
    paneContent.scrollTop = 480;

    view.rerender(<CommissionReviewViewer {...props} presentation="fullscreen" />);
    const focusedContent = view.container.querySelector('.viewer-content') as HTMLDivElement;
    expect(focusedContent).toBe(paneContent);
    expect(focusedContent.scrollTop).toBe(480);

    focusedContent.scrollTop = 760;
    view.rerender(<CommissionReviewViewer {...props} presentation="pane" />);
    expect(view.container.querySelector('.viewer-content')).toBe(paneContent);
    expect(paneContent.scrollTop).toBe(760);
  });

  it('owns the active focus scope while mounted', async () => {
    render(
      <FocusScopeProvider>
        <ActiveScope />
        <CommissionReviewViewer
          sequenceId={7}
          messages={[request]}
          onClose={() => {}}
          inline
        />
      </FocusScopeProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('active-focus-scope')).toHaveTextContent('commission-review-viewer');
    });
  });
});
