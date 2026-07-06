import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TaskApprovalReader } from './TaskApprovalReader';

function renderTaskApprovalReader(plan = '# Plan\n\nAdd the thing.', onApprove = vi.fn()) {
  return render(
    <TaskApprovalReader
      title="Review task"
      priority="p2"
      plan={plan}
      onApprove={onApprove}
      onReject={vi.fn()}
      onSendFeedback={vi.fn()}
    />
  );
}

function toolbarButtons() {
  const toolbar = document.querySelector('.task-approval-actions');
  if (!toolbar) throw new Error('toolbar not found');
  return within(toolbar as HTMLElement).getAllByRole('button');
}

describe('TaskApprovalReader markdown rendering', () => {
  it('renders fenced mermaid diagrams through the shared diagram component', async () => {
    const { container } = renderTaskApprovalReader([
      '# Plan',
      '',
      '```mermaid',
      'flowchart TD',
      '  A --> B',
      '```',
    ].join('\n'));

    expect(await screen.findByTestId('mermaid-diagram')).toBeInTheDocument();
    expect(container.querySelector('code.language-mermaid')).not.toBeInTheDocument();
  });
});

describe('TaskApprovalReader feedback action emphasis', () => {
  it('keeps the default approval-oriented toolbar with no notes', () => {
    renderTaskApprovalReader();

    const buttons = toolbarButtons();
    expect(buttons.map((button) => button.getAttribute('aria-label'))).toEqual([
      'Send Feedback (0)',
      'Continue here',
      'Start fresh conversation',
    ]);
    expect(screen.getByRole('button', { name: 'Discard task' })).toHaveClass('task-approval-header-discard');

    expect(buttons[0]).toBeDisabled();
    expect(buttons[0]).toHaveAttribute(
      'title',
      'Add annotations to the plan before sending feedback'
    );
    expect(buttons[0]).not.toHaveClass('task-approval-btn--recommended');
    expect(buttons[1]).toHaveClass('task-approval-btn--approve');
    expect(buttons[1]).not.toHaveClass('task-approval-btn--subdued');
    expect(buttons[2]).toHaveClass('task-approval-btn--approve');
    expect(buttons[2]).not.toHaveClass('task-approval-btn--subdued');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps action order stable and recommends sending feedback when notes exist', () => {
    renderTaskApprovalReader();

    fireEvent.click(screen.getByRole('button', { name: /add note to line 1/i }));
    fireEvent.change(screen.getByPlaceholderText('Add your note...'), {
      target: { value: 'Please adjust this plan.' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));

    const buttons = toolbarButtons();
    expect(buttons.map((button) => button.getAttribute('aria-label'))).toEqual([
      'Send Feedback (1)',
      'Continue here',
      'Start fresh conversation',
    ]);

    expect(buttons[0]).toBeEnabled();
    expect(buttons[0]).toHaveClass('task-approval-btn--recommended');
    expect(buttons[0]).toHaveAttribute('title', 'Send 1 note as feedback');
    expect(buttons[1]).toHaveClass('task-approval-btn--approve');
    expect(buttons[1]).toHaveClass('task-approval-btn--subdued');
    expect(buttons[2]).toHaveClass('task-approval-btn--approve');
    expect(buttons[2]).toHaveClass('task-approval-btn--subdued');
    expect(screen.getByRole('status')).toHaveTextContent(
      'You have 1 note of unsent feedback'
    );
  });

  it('calls onApprove with the selected handoff', () => {
    const onApprove = vi.fn();
    renderTaskApprovalReader(undefined, onApprove);

    fireEvent.click(screen.getByRole('button', { name: 'Continue here' }));
    expect(onApprove).toHaveBeenCalledWith('continue_in_current_conversation');

    expect(screen.getByRole('button', { name: 'Start fresh conversation' })).toBeInTheDocument();
  });
});
