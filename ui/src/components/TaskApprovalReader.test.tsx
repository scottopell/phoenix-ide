import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TaskApprovalReader } from './TaskApprovalReader';
import type { TaskApprovalReaderProps } from './TaskApprovalReader';

function renderTaskApprovalReader(
  plan = '# Plan\n\nAdd the thing.',
  onApprove = vi.fn(),
  overrides: Partial<TaskApprovalReaderProps> = {}
) {
  return render(
    <TaskApprovalReader
      title="Review task"
      priority="p2"
      plan={plan}
      onApprove={onApprove}
      onReject={vi.fn()}
      onSendFeedback={vi.fn()}
      {...overrides}
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

    expect(container.querySelector('[data-testid="mermaid-diagram"]')).not.toBeNull();
    expect(container.querySelector('code.language-mermaid')).not.toBeInTheDocument();
  });
});

describe('TaskApprovalReader feedback action emphasis', () => {
  it('keeps the default approval-oriented toolbar with no notes', () => {
    renderTaskApprovalReader();

    const buttons = toolbarButtons();
    expect(buttons.map((button) => button.getAttribute('aria-label'))).toEqual([
      'Request changes (0)',
      'Approve and start here',
      'Approve and start a new continuation conversation',
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
    renderTaskApprovalReader('# Plan\n\nAdd the thing.', vi.fn(), {
      contextWindowUsed: 176_000,
      modelContextWindow: 200_000,
    });

    fireEvent.click(screen.getByRole('button', { name: /add note to line 1/i }));
    fireEvent.change(screen.getByPlaceholderText('Add your note...'), {
      target: { value: 'Please adjust this plan.' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));

    const buttons = toolbarButtons();
    expect(buttons.map((button) => button.getAttribute('aria-label'))).toEqual([
      'Request changes (1)',
      'Approve and start here',
      'Approve and start a new continuation conversation',
    ]);

    expect(buttons[0]).toBeEnabled();
    expect(buttons[0]).toHaveClass('task-approval-btn--recommended');
    expect(buttons[0]).toHaveAttribute('title', 'Send 1 note as feedback');
    expect(buttons[1]).toHaveClass('task-approval-btn--approve');
    expect(buttons[1]).toHaveClass('task-approval-btn--subdued');
    expect(buttons[2]).toHaveClass('task-approval-btn--approve');
    expect(buttons[2]).toHaveClass('task-approval-btn--subdued');
    expect(buttons[1]).not.toHaveClass('task-approval-btn--recommended-decision');
    expect(buttons[2]).not.toHaveClass('task-approval-btn--recommended-decision');
    expect(screen.getByRole('status')).toHaveTextContent(
      'You have 1 note of unsent feedback'
    );
  });

  it('shows context usage and recommends the lower-pressure start path', () => {
    renderTaskApprovalReader('# Plan', vi.fn(), {
      contextWindowUsed: 96_000,
      modelContextWindow: 200_000,
    });

    expect(screen.getByText('Context')).toBeInTheDocument();
    expect(screen.getByText('48% used')).toBeInTheDocument();
    expect(screen.getByText('Start here recommended')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Approve and start here' }))
      .toHaveClass('task-approval-btn--recommended-decision');
  });

  it('recommends a new chat when context pressure is high', () => {
    renderTaskApprovalReader('# Plan', vi.fn(), {
      contextWindowUsed: 176_000,
      modelContextWindow: 200_000,
    });

    expect(screen.getByText('88% used')).toBeInTheDocument();
    expect(screen.getByText('New chat recommended')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Approve and start a new continuation conversation' }))
      .toHaveClass('task-approval-btn--recommended-decision');
  });

  it('calls onApprove with the selected handoff', () => {
    const onApprove = vi.fn();
    renderTaskApprovalReader(undefined, onApprove);

    fireEvent.click(screen.getByRole('button', { name: 'Approve and start here' }));
    expect(onApprove).toHaveBeenCalledWith('continue_in_current_conversation');

    expect(screen.getByRole('button', { name: 'Approve and start a new continuation conversation' })).toBeInTheDocument();
  });

  it('clears approving state and shows async approval errors', () => {
    const onApprove = vi.fn();
    const { rerender } = renderTaskApprovalReader(undefined, onApprove);

    fireEvent.click(screen.getByRole('button', { name: 'Approve and start here' }));
    expect(screen.getByText('Approving...')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Approve and start a new continuation conversation' })).toBeDisabled();

    rerender(
      <TaskApprovalReader
        title="Review task"
        priority="p2"
        plan="# Plan\n\nAdd the thing."
        approvalError="Target branch already exists"
        onApprove={onApprove}
        onReject={vi.fn()}
        onSendFeedback={vi.fn()}
      />
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Target branch already exists');
    expect(screen.queryByText('Approving...')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Approve and start here' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Approve and start a new continuation conversation' })).toBeEnabled();
  });
});


describe('TaskApprovalReader shared find integration', () => {
  it('opens shared find and restores focus to the opener on Escape', async () => {
    renderTaskApprovalReader('# Plan\n\nalpha\nbeta alpha');

    const findButton = screen.getByRole('button', { name: 'Find in task approval' });
    findButton.focus();
    fireEvent.click(findButton);

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });

    await waitFor(() => expect(input).toHaveValue('alpha'));

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(findButton).toHaveFocus();
  });

  it('preserves original task text when overlapping find fragments are highlighted', async () => {
    renderTaskApprovalReader('# Plan\n\nbanana');

    fireEvent.click(screen.getByRole('button', { name: 'Find in task approval' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'ana' } });

    await waitFor(() => expect(screen.getByText('banana')).toBeInTheDocument());
    await waitFor(() => expect(document.querySelectorAll('mark').length).toBe(1));
  });

  it('lets find Escape close the find bar without dismissing the approval reader or note dialog precedence', () => {
    renderTaskApprovalReader('# Plan\n\nalpha');

    fireEvent.click(screen.getByRole('button', { name: 'Find in task approval' }));
    expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /add note to line 1/i }));
    expect(screen.getByPlaceholderText('Add your note...')).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toBeInTheDocument();
    expect(screen.queryByPlaceholderText('Add your note...')).not.toBeInTheDocument();
    expect(screen.getByText('Review task')).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).not.toBeInTheDocument();
    expect(screen.getByText('Review task')).toBeInTheDocument();
  });

  it('leaves repeated Ctrl/Cmd+F routed to the existing task find input', async () => {
    renderTaskApprovalReader('# Plan\n\nalpha\nbeta alpha');

    fireEvent.click(screen.getByRole('button', { name: 'Find in task approval' }));
    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });

    expect(input).toHaveFocus();
    fireEvent.keyDown(window, { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    await waitFor(() => expect(input).toHaveFocus());
    expect((input as HTMLInputElement).selectionStart).toBe((input as HTMLInputElement).selectionEnd);
  });

  it('does not open find behind task-approval dialogs', () => {
    renderTaskApprovalReader('# Plan\n\nalpha');

    fireEvent.click(screen.getByRole('button', { name: /add note to line 1/i }));
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    fireEvent.keyDown(cancel, { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
  });

  it('clears task-plan marks when find closes', async () => {
    renderTaskApprovalReader('# Plan\n\nbanana');

    fireEvent.click(screen.getByRole('button', { name: 'Find in task approval' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'ana' } });
    await waitFor(() => expect(document.querySelectorAll('mark').length).toBe(1));

    fireEvent.click(screen.getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(document.querySelector('mark')).toBeNull();
  });

  it('renders inline-code and fenced-code text visibly in the task plan surface', () => {
    renderTaskApprovalReader([
      '# Plan',
      '',
      'Use `alpha` inline and [alpha link](https://example.com).',
      '',
      '```ts',
      'const alpha = true;',
      '```',
    ].join('\n'));

    expect(screen.getByText('alpha link')).toBeInTheDocument();
    expect(screen.getByText('const')).toBeInTheDocument();
    expect(screen.getByText('true')).toBeInTheDocument();
  });

  it('drives task find counts and active navigation from projected markdown blocks', async () => {
    renderTaskApprovalReader([
      '# Plan',
      '',
      'Use **alpha** in prose.',
      '',
      '```ts',
      'const alpha = true;',
      '```',
    ].join('\n'));

    fireEvent.click(screen.getByRole('button', { name: 'Find in task approval' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
  });

  it('renders only one notes badge when notes exist', () => {
    renderTaskApprovalReader('# Plan\n\nAdd the thing.');

    fireEvent.click(screen.getByRole('button', { name: /add note to line 1/i }));
    fireEvent.change(screen.getByPlaceholderText('Add your note...'), {
      target: { value: 'Please adjust this plan.' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));

    expect(screen.getAllByRole('button', { name: '1 notes' })).toHaveLength(1);
  });
});
