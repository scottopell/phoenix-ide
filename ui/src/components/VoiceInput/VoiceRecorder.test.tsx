import { createRef } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { VoiceRecorder, type VoiceRecorderHandle } from './VoiceRecorder';

type Recognition = InstanceType<NonNullable<Window['SpeechRecognition']>>;
type ResultEvent = Parameters<NonNullable<Recognition['onresult']>>[0];

function resultEvent(...entries: { transcript: string; isFinal: boolean }[]): ResultEvent {
  const results: ResultEvent['results'][number][] = entries.map(({ transcript, isFinal }) => {
    const alternatives = [{ transcript, confidence: 1 }];
    return Object.assign(alternatives, { isFinal, item: (index: number) => alternatives[index]! });
  });
  return Object.assign(new Event('result'), {
    resultIndex: 0,
    results: Object.assign(results, { item: (index: number) => results[index]! }),
  });
}

class FakeSpeechRecognition extends EventTarget implements Recognition {
  static instances: FakeSpeechRecognition[] = [];
  continuous = false;
  interimResults = false;
  maxAlternatives = 0;
  lang = '';
  onstart: Recognition['onstart'] = null;
  onend: Recognition['onend'] = null;
  onerror: Recognition['onerror'] = null;
  onresult: Recognition['onresult'] = null;
  start = vi.fn();
  stop = vi.fn();
  abort = vi.fn();

  constructor() {
    super();
    FakeSpeechRecognition.instances.push(this);
  }

  started() {
    this.onstart?.call(this, new Event('start'));
  }

  result(transcript: string, isFinal = false) {
    this.onresult?.call(this, resultEvent({ transcript, isFinal }));
  }

  error(error: string) {
    this.onerror?.call(this, Object.assign(new Event('error'), { error, message: error }));
  }

  end() {
    this.onend?.call(this, new Event('end'));
  }
}

function mountRecorder() {
  const ref = createRef<VoiceRecorderHandle>();
  const callbacks = {
    onStart: vi.fn(),
    onEnd: vi.fn(),
    onSpeech: vi.fn(),
    onInterim: vi.fn(),
  };
  const view = render(<VoiceRecorder ref={ref} {...callbacks} />);
  return { ...view, ref, callbacks };
}

function startRecording() {
  fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
  const recognition = FakeSpeechRecognition.instances.at(-1)!;
  act(() => recognition.started());
  return recognition;
}

function clearCallbacks(callbacks: ReturnType<typeof mountRecorder>['callbacks']) {
  Object.values(callbacks).forEach(callback => callback.mockClear());
}

function expectNoCallbacks(callbacks: ReturnType<typeof mountRecorder>['callbacks']) {
  Object.values(callbacks).forEach(callback => expect(callback).not.toHaveBeenCalled());
}

function staleEvents(recognition: FakeSpeechRecognition) {
  recognition.result('stale final', true);
  recognition.result('stale interim');
  recognition.error('not-allowed');
  recognition.error('no-speech');
  recognition.end();
  recognition.started();
}

describe('VoiceRecorder session identity', () => {
  beforeEach(() => {
    FakeSpeechRecognition.instances = [];
    vi.stubGlobal('SpeechRecognition', FakeSpeechRecognition);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('claims ownership before recognition.start and never repeats it on browser start events', () => {
    const { callbacks } = mountRecorder();
    callbacks.onStart.mockImplementation(() => {
      expect(FakeSpeechRecognition.instances.at(-1)!.start).not.toHaveBeenCalled();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
    const recognition = FakeSpeechRecognition.instances.at(-1)!;
    expect(callbacks.onStart).toHaveBeenCalledTimes(1);
    expect(recognition.start).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Requesting permission...' })).toBeDisabled();

    act(() => recognition.started());
    act(() => recognition.started());
    expect(callbacks.onStart).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Stop recording' })).toBeEnabled();
    act(() => recognition.end());
    expect(callbacks.onEnd).toHaveBeenCalledTimes(1);
  });

  it('releases a failed start after invalidating synchronous abort callbacks', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const { callbacks } = mountRecorder();
    vi.stubGlobal('SpeechRecognition', class extends FakeSpeechRecognition {
      override start = vi.fn(() => { throw new Error('Start failed'); });
      override abort = vi.fn((): void => staleEvents(this));
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
    const recognition = FakeSpeechRecognition.instances.at(-1)!;
    expect(callbacks.onStart).toHaveBeenCalledTimes(1);
    expect(callbacks.onEnd).toHaveBeenCalledTimes(1);
    expect(recognition.abort).toHaveBeenCalledTimes(1);
    expect(callbacks.onSpeech).not.toHaveBeenCalled();
    expect(callbacks.onInterim).not.toHaveBeenCalled();
    clearCallbacks(callbacks);
    act(() => staleEvents(recognition));
    expectNoCallbacks(callbacks);
    expect(screen.getByRole('button', { name: 'Voice input error - click to retry' })).toBeEnabled();
  });

  it('ends a cancelled request once without waiting for a browser start', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const { callbacks } = mountRecorder();
    fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
    const recognition = FakeSpeechRecognition.instances.at(-1)!;
    act(() => {
      recognition.error('aborted');
      recognition.end();
    });
    expect(callbacks.onStart).toHaveBeenCalledTimes(1);
    expect(callbacks.onEnd).toHaveBeenCalledTimes(1);
    clearCallbacks(callbacks);
    act(() => staleEvents(recognition));
    expectNoCallbacks(callbacks);
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
  });

  it('does not start a session retired synchronously by its ownership callback', () => {
    const { ref, callbacks } = mountRecorder();
    callbacks.onStart.mockImplementation(() => ref.current!.retire());
    fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
    const recognition = FakeSpeechRecognition.instances.at(-1)!;
    expect(recognition.start).not.toHaveBeenCalled();
    expect(recognition.abort).toHaveBeenCalledTimes(1);
    expect(callbacks.onEnd).not.toHaveBeenCalled();
    clearCallbacks(callbacks);
    act(() => staleEvents(recognition));
    expectNoCallbacks(callbacks);
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
  });

  it('retires submission synchronously before abort callbacks, without publishing text or end', () => {
    const { ref, callbacks } = mountRecorder();
    const old = startRecording();
    act(() => old.result('submitted interim'));
    clearCallbacks(callbacks);
    old.abort.mockImplementationOnce(() => staleEvents(old));

    act(() => ref.current!.retire());

    expect(old.abort).toHaveBeenCalledTimes(1);
    expectNoCallbacks(callbacks);
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
    act(() => staleEvents(old));
    expectNoCallbacks(callbacks);
    expect(screen.queryByText(/Microphone access was denied/)).not.toBeInTheDocument();

    const current = startRecording();
    act(() => current.result('next draft'));
    expect(callbacks.onInterim).toHaveBeenLastCalledWith('next draft');
    act(() => current.result('next final', true));
    expect(callbacks.onSpeech).toHaveBeenCalledExactlyOnceWith('next final');
  });

  it.each(['submitted', 'ended', 'no-speech', 'retry'] as const)(
    'ignores all old callbacks during a new recording after the old session is %s',
    transition => {
      const { ref, callbacks } = mountRecorder();
      const old = startRecording();
      if (transition === 'submitted') {
        act(() => ref.current!.retire());
      } else if (transition === 'ended') {
        act(() => old.end());
      } else {
        vi.spyOn(console, 'error').mockImplementation(() => {});
        act(() => old.error(transition === 'retry' ? 'not-allowed' : 'no-speech'));
      }
      old.abort.mockImplementationOnce(() => staleEvents(old));
      clearCallbacks(callbacks);
      let current: FakeSpeechRecognition;
      if (transition === 'retry') {
        fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
        current = FakeSpeechRecognition.instances.at(-1)!;
        act(() => current.started());
      } else {
        current = startRecording();
      }
      expect(callbacks.onStart).toHaveBeenCalledTimes(1);
      act(() => current.result('current interim'));
      clearCallbacks(callbacks);

      act(() => staleEvents(old));

      expectNoCallbacks(callbacks);
      expect(screen.getByRole('button', { name: 'Stop recording' })).toHaveAttribute('aria-pressed', 'true');
      expect(screen.queryByText(/Microphone access was denied/)).not.toBeInTheDocument();
      // An identical interim must remain deduplicated after stale callbacks.
      act(() => current.result('current interim'));
      expect(callbacks.onInterim).not.toHaveBeenCalled();

      fireEvent.click(screen.getByRole('button', { name: 'Stop recording' }));
      expect(current.stop).toHaveBeenCalledTimes(1);
      expect(old.stop).not.toHaveBeenCalled();
      expect(screen.getByRole('button', { name: 'Processing speech...' })).toBeDisabled();
      act(() => current.result('current final', true));
      expect(callbacks.onInterim).toHaveBeenCalledExactlyOnceWith('');
      expect(callbacks.onSpeech).toHaveBeenCalledExactlyOnceWith('current final');
      act(() => current.end());
      expect(callbacks.onEnd).toHaveBeenCalledTimes(1);
      expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
    },
  );

  it('invalidates before unmount abort and ignores callbacks delivered after unmount', () => {
    const { unmount, callbacks } = mountRecorder();
    const recognition = startRecording();
    act(() => recognition.result('unfinished'));
    recognition.abort.mockImplementationOnce(() => staleEvents(recognition));
    clearCallbacks(callbacks);

    unmount();
    act(() => staleEvents(recognition));

    expect(recognition.abort).toHaveBeenCalledTimes(1);
    expectNoCallbacks(callbacks);
  });

  it('does not retire a current recording when the component rerenders', () => {
    const { rerender, ref, callbacks } = mountRecorder();
    const recognition = startRecording();
    rerender(<VoiceRecorder ref={ref} {...callbacks} />);
    act(() => recognition.result('still recording'));
    expect(recognition.abort).not.toHaveBeenCalled();
    expect(callbacks.onInterim).toHaveBeenCalledExactlyOnceWith('still recording');
    expect(screen.getByRole('button', { name: 'Stop recording' })).toBeEnabled();
  });

  it('retires an ended session before end callbacks can deliver more recognition events', () => {
    const { callbacks } = mountRecorder();
    const recognition = startRecording();
    callbacks.onEnd.mockImplementation(() => staleEvents(recognition));
    clearCallbacks(callbacks);
    act(() => recognition.end());
    expect(callbacks.onEnd).toHaveBeenCalledTimes(1);
    expect(callbacks.onSpeech).not.toHaveBeenCalled();
    expect(callbacks.onInterim).not.toHaveBeenCalled();
    expect(callbacks.onStart).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
  });

  it('stops processing a result event if a transcript callback retires the session', () => {
    const { ref, callbacks } = mountRecorder();
    const recognition = startRecording();
    callbacks.onSpeech.mockImplementation(() => ref.current!.retire());
    const event = resultEvent(
      { isFinal: true, transcript: 'submitted final' },
      { isFinal: false, transcript: 'must not leak' },
    );

    act(() => recognition.onresult?.call(recognition, event));

    expect(callbacks.onSpeech).toHaveBeenCalledExactlyOnceWith('submitted final');
    expect(callbacks.onInterim).not.toHaveBeenCalled();
    expect(recognition.abort).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
  });
});
