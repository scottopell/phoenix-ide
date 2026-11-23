import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import {
  receiveCoordinatorQuestions,
  resetCoordinatorQuestionsForTest,
} from '../coordinator/coordinatorQuestions';
import { FocusScopeProvider, useFocusScope } from '../hooks/useFocusScope';
import { CoordinatorQuestionDock } from './CoordinatorQuestionDock';
import { EmberEntry } from './EmberEntry';

const originalMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia');
const originalVisualViewport = Object.getOwnPropertyDescriptor(window, 'visualViewport');
const originalShow = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'show');
const originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'showModal');
const originalClose = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, 'close');

let mobile = false;
let mediaListeners: Set<() => void>;
let showDialog: ReturnType<typeof vi.fn>;
let showModal: ReturnType<typeof vi.fn>;

function setMobile(next: boolean): void {
  mobile = next;
  act(() => mediaListeners.forEach((listener) => listener()));
}

function mockAnimationFrames(): () => void {
  const frames = new Map<number, FrameRequestCallback>();
  let nextId = 0;
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
    frames.set(++nextId, callback);
    return nextId;
  });
  vi.spyOn(window, 'cancelAnimationFrame').mockImplementation((id) => {
    frames.delete(id);
  });
  return () => {
    act(() => {
      const pending = [...frames.values()];
      frames.clear();
      for (const callback of pending) callback(0);
    });
  };
}

function restoreProperty(target: object, key: PropertyKey, descriptor: PropertyDescriptor | undefined): void {
  if (descriptor) Object.defineProperty(target, key, descriptor);
  else Reflect.deleteProperty(target, key);
}

function ActiveScope() {
  const { activeScope } = useFocusScope();
  return <output data-testid="active-scope">{activeScope ?? 'none'}</output>;
}

function open(requestId: string, text: string): CoordinatorQuestion {
  return {
    request_id: requestId,
    coordinator_product_conversation_id: 'ember-1',
    asked_at: new Date().toISOString(),
    questions: [{
      question: text,
      header: 'Choice',
      options: [
        { label: 'First', description: 'First option' },
        { label: 'Second', description: 'Second option' },
      ],
      multiSelect: false,
    }],
  };
}

beforeEach(() => {
  mobile = false;
  mediaListeners = new Set();
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: vi.fn((query: string) => ({
      get matches() { return query === '(max-width: 768px)' && mobile; },
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: (_type: string, listener: () => void) => mediaListeners.add(listener),
      removeEventListener: (_type: string, listener: () => void) => mediaListeners.delete(listener),
      dispatchEvent: vi.fn(),
    })),
  });

  showDialog = vi.fn(function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  });
  showModal = vi.fn(function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  });
  Object.defineProperty(HTMLDialogElement.prototype, 'show', { configurable: true, value: showDialog });
  Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value: showModal });
  Object.defineProperty(HTMLDialogElement.prototype, 'close', {
    configurable: true,
    value: vi.fn(function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    }),
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  resetCoordinatorQuestionsForTest();
  restoreProperty(window, 'matchMedia', originalMatchMedia);
  restoreProperty(window, 'visualViewport', originalVisualViewport);
  restoreProperty(HTMLDialogElement.prototype, 'show', originalShow);
  restoreProperty(HTMLDialogElement.prototype, 'showModal', originalShowModal);
  restoreProperty(HTMLDialogElement.prototype, 'close', originalClose);
});

describe('Ember questions (REQ-AUQ-011)', () => {
  it('shows the oldest open question and sends the answer to Ember, not to a conversation wait', async () => {
    const answer = vi.spyOn(api, 'answerCoordinatorQuestion').mockResolvedValue({ success: true });
    const respond = vi.spyOn(api, 'respondToQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?'), open('q-2', 'Deploy tonight?')]));
    render(<CoordinatorQuestionDock />);

    expect(screen.getByText('Merge now?')).toBeInTheDocument();
    expect(screen.getByText('1 more waiting')).toBeInTheDocument();
    expect(screen.getByText(/Ember keeps working while you decide/)).toBeInTheDocument();
    expect(showDialog).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));

    await waitFor(() => expect(answer).toHaveBeenCalledWith('q-1', { 'Merge now?': 'First' }, undefined));
    expect(respond).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByText('Deploy tonight?')).toBeInTheDocument());
    expect(screen.queryByText('1 more waiting')).not.toBeInTheDocument();
  });

  it('keeps desktop question presentation open after a native cancel request', () => {
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    const cancel = new Event('cancel', { cancelable: true });
    fireEvent(dialog, cancel);

    expect(cancel.defaultPrevented).toBe(true);
    expect(dialog).toHaveAttribute('open');
  });

  it('dismisses through the Ember question endpoint', async () => {
    const dismiss = vi.spyOn(api, 'dismissCoordinatorQuestion').mockResolvedValue({ success: true });
    const dismissWait = vi.spyOn(api, 'dismissQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.getByText(/Ember is told you dismissed it/)).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[1]!);

    await waitFor(() => expect(dismiss).toHaveBeenCalledWith('q-1'));
    expect(dismissWait).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByText('Merge now?')).not.toBeInTheDocument());
  });

  it('uses a native mobile modal and returns without unmounting or losing the selection', async () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog', { name: 'Ember has a question' });
    const panel = document.querySelector('.question-panel');
    expect(showModal).toHaveBeenCalledOnce();
    await waitFor(() => expect(panel).toHaveFocus());

    fireEvent.click(screen.getByTitle('Select First'));
    expect(screen.getByRole('button', { name: 'Submit' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));

    const launcher = screen.getByRole('button', { name: 'Answer Ember’s question' });
    await waitFor(() => expect(launcher).toHaveFocus());
    expect(dialog).not.toHaveAttribute('open');
    expect(document.querySelector('.question-panel')).toBe(panel);

    fireEvent.click(launcher);
    await waitFor(() => expect(dialog).toHaveAttribute('open'));
    expect(document.querySelector('.question-panel')).toBe(panel);
    expect(screen.getByTitle('Select First')).toHaveClass('selected');
    expect(screen.getByRole('button', { name: 'Submit' })).toBeEnabled();
  });

  it('clears a dismiss confirmation on Return and retains the selection when reopened', async () => {
    mobile = true;
    const dismiss = vi.spyOn(api, 'dismissCoordinatorQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    const panel = document.querySelector('.question-panel');
    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.getByText('Dismiss structured questions?')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
    expect(dialog).not.toHaveAttribute('open');
    expect(screen.queryByText('Dismiss structured questions?')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Answer Ember’s question' }));
    await waitFor(() => expect(panel).toHaveFocus());
    expect(dialog).toHaveAttribute('open');
    expect(document.querySelector('.question-panel')).toBe(panel);
    expect(screen.queryByText('Dismiss structured questions?')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Cancel', exact: true })).not.toBeInTheDocument();
    expect(screen.getByTitle('Select First')).toHaveClass('selected');
    expect(screen.getByRole('button', { name: 'Submit' })).toBeEnabled();
    expect(dismiss).not.toHaveBeenCalled();
  });

  it('does not let the pending mobile focus frame steal focus from confirmation Cancel', () => {
    const flushFrames = mockAnimationFrames();
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    const cancel = screen.getByRole('button', { name: 'Cancel', exact: true });
    expect(cancel).toHaveFocus();

    flushFrames();

    expect(cancel).toHaveFocus();
    expect(dialog).toHaveAttribute('open');
    expect(dialog).toContainElement(cancel);
    expect(showModal).toHaveBeenCalledOnce();
  });

  it('refocuses confirmation Cancel when crossing from desktop to the mobile modal', () => {
    const flushFrames = mockAnimationFrames();
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    const panel = document.querySelector('.question-panel');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    const cancel = screen.getByRole('button', { name: 'Cancel', exact: true });
    expect(cancel).toHaveFocus();
    showModal.mockImplementationOnce(function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
      this.querySelector<HTMLElement>('.question-panel')?.focus();
    });

    setMobile(true);

    expect(showModal).toHaveBeenCalledOnce();
    expect(screen.getByRole('dialog')).toBe(dialog);
    expect(dialog).toHaveAttribute('open');
    expect(document.querySelector('.question-panel')).toBe(panel);
    expect(dialog).toContainElement(cancel);
    expect(cancel).toHaveFocus();
    flushFrames();
    expect(cancel).toHaveFocus();

    fireEvent.click(cancel);
    expect(screen.queryByText('Dismiss structured questions?')).not.toBeInTheDocument();
    expect(dialog).toHaveAttribute('open');
    expect(screen.queryByRole('button', { name: 'Answer Ember’s question' })).not.toBeInTheDocument();
    expect(showModal).toHaveBeenCalledOnce();
  });

  it('treats Escape and the native cancel request as Return, never as Dismiss', async () => {
    mobile = true;
    const dismiss = vi.spyOn(api, 'dismissCoordinatorQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.getByRole('button', { name: 'Answer Ember’s question' })).toBeInTheDocument();
    expect(dismiss).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Answer Ember’s question' }));
    const routeBeforeCancel = window.location.href;
    fireEvent(dialog, new Event('cancel', { cancelable: true }));
    expect(window.location.href).toBe(routeBeforeCancel);
    expect(screen.getByRole('button', { name: 'Answer Ember’s question' })).toBeInTheDocument();
    expect(dismiss).not.toHaveBeenCalled();
  });

  it('native cancel cancels the nested confirmation without returning from the question', () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.getByText('Dismiss structured questions?')).toBeInTheDocument();

    const cancel = new Event('cancel', { cancelable: true });
    fireEvent(dialog, cancel);

    expect(cancel.defaultPrevented).toBe(true);
    expect(screen.queryByText('Dismiss structured questions?')).not.toBeInTheDocument();
    expect(dialog).toHaveAttribute('open');
    expect(screen.queryByRole('button', { name: 'Answer Ember’s question' })).not.toBeInTheDocument();
  });

  it('lets QuestionPanel blur and refocus from the first Escape in its input', async () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    const input = screen.getByRole('textbox', { name: 'Write a custom answer' });
    const panel = document.querySelector('.question-panel');
    input.focus();
    fireEvent.keyDown(input, { key: 'Escape' });

    expect(panel).toHaveFocus();
    expect(dialog).toHaveAttribute('open');
    expect(screen.queryByRole('button', { name: 'Answer Ember’s question' })).not.toBeInTheDocument();
  });

  it('releases the question focus scope when Return closes the mounted panel', async () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(
      <FocusScopeProvider>
        <ActiveScope />
        <CoordinatorQuestionDock />
      </FocusScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('active-scope')).toHaveTextContent('question-panel'));
    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
    await waitFor(() => expect(screen.getByTestId('active-scope')).toHaveTextContent('none'));
  });

  it('keeps the dismiss confirmation inside the mobile dialog and lets Escape cancel only the confirmation', () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(dialog).toContainElement(screen.getByText('Dismiss structured questions?'));

    fireEvent.keyDown(screen.getByText('Dismiss structured questions?'), { key: 'Escape' });
    expect(screen.queryByText('Dismiss structured questions?')).not.toBeInTheDocument();
    expect(dialog).toHaveAttribute('open');
    expect(screen.queryByRole('button', { name: 'Answer Ember’s question' })).not.toBeInTheDocument();
  });

  it('does not reopen a returned same-id poll, but auto-opens a replacement request', async () => {
    mobile = true;
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
    act(() => receiveCoordinatorQuestions([
      open('q-1', 'Updated copy that must not replace the posted request'),
      open('q-2', 'Another request waiting behind it'),
    ]));
    expect(screen.getByRole('button', { name: 'Answer Ember’s question' })).toBeInTheDocument();
    expect(screen.getByText('+1')).toBeInTheDocument();
    expect(showModal).toHaveBeenCalledOnce();

    act(() => receiveCoordinatorQuestions([open('q-3', 'Deploy tonight?')]));
    await waitFor(() => expect(screen.getByRole('dialog')).toHaveAttribute('open'));
    expect(screen.getByText('Deploy tonight?')).toBeInTheDocument();
    expect(showModal).toHaveBeenCalledTimes(2);
  });

  it('keeps one QuestionPanel instance and its selection across desktop/mobile changes', async () => {
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);
    const panel = document.querySelector('.question-panel');

    fireEvent.click(screen.getByTitle('Select First'));
    setMobile(true);
    await waitFor(() => expect(showModal).toHaveBeenCalledOnce());
    expect(document.querySelector('.question-panel')).toBe(panel);
    expect(screen.getByTitle('Select First')).toHaveClass('selected');

    setMobile(false);
    await waitFor(() => expect(showDialog).toHaveBeenCalledTimes(2));
    expect(document.querySelector('.question-panel')).toBe(panel);
    expect(screen.getByTitle('Select First')).toHaveClass('selected');
  });

  it('fits the mobile dialog to visualViewport changes from the software keyboard', () => {
    mobile = true;
    const viewportListeners = new Map<string, () => void>();
    const viewport = {
      offsetTop: 12,
      offsetLeft: 3,
      width: 390,
      height: 500,
      addEventListener: (type: string, listener: () => void) => viewportListeners.set(type, listener),
      removeEventListener: (type: string) => viewportListeners.delete(type),
    };
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport });
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    const dialog = screen.getByRole('dialog');
    expect(dialog.style.getPropertyValue('--coordinator-question-viewport-height')).toBe('500px');
    viewport.height = 320;
    act(() => viewportListeners.get('resize')?.());
    expect(dialog.style.getPropertyValue('--coordinator-question-viewport-height')).toBe('320px');
  });

  it('leaves a replacement request intact when the replaced request finishes asynchronously', async () => {
    let finishAnswer!: () => void;
    const answer = vi.spyOn(api, 'answerCoordinatorQuestion').mockReturnValue(new Promise((resolve) => {
      finishAnswer = () => resolve({ success: true });
    }));
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    await waitFor(() => expect(answer).toHaveBeenCalledOnce());
    act(() => receiveCoordinatorQuestions([open('q-2', 'Deploy tonight?')]));
    expect(screen.getByText('Deploy tonight?')).toBeInTheDocument();

    await act(async () => finishAnswer());
    expect(screen.getByText('Deploy tonight?')).toBeInTheDocument();
  });

  it('keeps the request actionable after submit and dismiss failures', async () => {
    const answer = vi.spyOn(api, 'answerCoordinatorQuestion').mockRejectedValueOnce(new Error('Answer failed'));
    const dismiss = vi.spyOn(api, 'dismissCoordinatorQuestion').mockRejectedValueOnce(new Error('Dismiss failed'));
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    expect(await screen.findByText('Answer failed')).toBeInTheDocument();
    expect(screen.getByText('Merge now?')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[1]!);
    expect(await screen.findByText('Dismiss failed')).toBeInTheDocument();
    expect(screen.getByText('Merge now?')).toBeInTheDocument();
    expect(answer).toHaveBeenCalledOnce();
    expect(dismiss).toHaveBeenCalledOnce();
  });

  it('marks the Ember entry while a question waits', () => {
    const { rerender } = render(<EmberEntry onClick={() => {}} />);
    expect(screen.getByRole('button', { name: 'Open Ember' })).toBeInTheDocument();

    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?'), open('q-2', 'Deploy?')]));
    rerender(<EmberEntry onClick={() => {}} />);
    expect(screen.getByRole('button', { name: 'Open Ember, 2 questions for you' })).toBeInTheDocument();
    expect(screen.getByText('2 questions for you')).toBeInTheDocument();
  });
});
