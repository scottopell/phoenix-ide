import { useState } from 'react';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ConnectedInputArea } from './InputArea';
import { ExpansionError, type FileAttachment, type ImageData } from '../api';
import { DraftStore } from '../conversation/DraftStore';
import { DraftContext } from '../conversation/DraftContext';

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

function Harness({ store, send }: {
  store: DraftStore;
  send: (text: string, images: ImageData[], files: FileAttachment[]) => Promise<void>;
}) {
  const [images, setImages] = useState<ImageData[]>([]);
  const [files, setFiles] = useState<FileAttachment[]>([]);
  return <DraftContext.Provider value={store}>
    <ConnectedInputArea slug="conversation" scopeKey="conversation" cwd={undefined}
      convState={{ type: 'idle' }} images={images} setImages={setImages}
      files={files} setFiles={setFiles} isOffline={false} failedMessages={[]}
      onSend={send} onCancel={() => {}} onRetry={() => {}} />
  </DraftContext.Provider>;
}

function startRecording() {
  fireEvent.click(screen.getByRole('button', { name: 'Start voice input' }));
  return FakeSpeechRecognition.instances.at(-1)!;
}

describe('InputArea with real VoiceRecorder submission ownership', () => {
  beforeEach(() => {
    FakeSpeechRecognition.instances = [];
    vi.stubGlobal('SpeechRecognition', FakeSpeechRecognition);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it.each(['start failure', 'cancelled request'] as const)('releases composer ownership after a %s', async outcome => {
    const store = new DraftStore();
    store.dispatch('conversation', { type: 'set_draft', text: '@missing.ts' });
    let reject!: (error: Error) => void;
    const pending = new Promise<void>((_resolve, rejectPromise) => { reject = rejectPromise; });
    render(<Harness store={store} send={vi.fn(() => pending)} />);
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));

    if (outcome === 'start failure') {
      vi.spyOn(console, 'error').mockImplementation(() => {});
      vi.stubGlobal('SpeechRecognition', class extends FakeSpeechRecognition {
        override start = vi.fn(() => { throw new Error('Start failed'); });
      });
    }
    const recognition = startRecording();
    if (outcome === 'cancelled request') {
      vi.spyOn(console, 'error').mockImplementation(() => {});
      act(() => {
        recognition.error('aborted');
        recognition.end();
      });
    }

    await act(async () => reject(new ExpansionError({
      error: 'Missing file', error_type: 'file_not_found', reference: '@missing.ts',
    })));
    expect(store.getRejected('conversation')).toEqual([
      expect.objectContaining({ text: '@missing.ts' }),
    ]);
    fireEvent.click(screen.getByText('Unsent message'));
    expect(screen.getByRole('button', { name: 'Restore unsent message' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Restore unsent message' }));
    expect(screen.getByRole('textbox')).toHaveValue('@missing.ts');
    expect(store.getSnapshot('conversation').draft).toBe('@missing.ts');
    act(() => {
      recognition.started();
      recognition.result('stale text', true);
      recognition.end();
    });
    expect(screen.getByRole('textbox')).toHaveValue('@missing.ts');
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'editable after request' } });
    expect(store.getSnapshot('conversation').draft).toBe('editable after request');
  });

  it('retains rejected recording A separately while recording B survives stale A callbacks and ends normally', async () => {
    const store = new DraftStore();
    store.dispatch('conversation', { type: 'set_draft', text: '@missing.ts' });
    let reject!: (error: Error) => void;
    const pending = new Promise<void>((_resolve, rejectPromise) => { reject = rejectPromise; });
    const send = vi.fn(() => pending);
    render(<Harness store={store} send={send} />);

    const a = startRecording();
    act(() => a.started());
    act(() => a.result('A final', true));
    act(() => a.result('A interim'));
    expect(screen.getByRole('textbox')).toHaveValue('@missing.ts A final A interim');
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
    expect(send).toHaveBeenCalledExactlyOnceWith('@missing.ts A final A interim', [], []);
    expect(a.abort).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('textbox')).toHaveValue('');

    const b = startRecording();
    expect(screen.getByRole('button', { name: 'Requesting permission...' })).toBeDisabled();
    await act(async () => reject(new ExpansionError({
      error: 'Missing file', error_type: 'file_not_found', reference: '@missing.ts',
    })));
    expect(screen.getByRole('textbox')).toHaveValue('');
    expect(store.getRejected('conversation')).toEqual([
      expect.objectContaining({ text: '@missing.ts A final A interim', images: [], files: [] }),
    ]);
    fireEvent.click(screen.getByText('Unsent message'));
    expect(screen.getByRole('button', { name: 'Restore unsent message' })).toBeDisabled();
    const unsent = screen.getByText('Unsent message').closest('details')!;
    expect(within(unsent).getByText('@missing.ts A final A interim')).toBeInTheDocument();

    act(() => b.started());
    act(() => b.result('B interim'));
    expect(screen.getByRole('textbox')).toHaveValue('B interim');
    act(() => b.started());
    expect(screen.getByRole('textbox')).toHaveValue('B interim');

    act(() => {
      a.result('stale A final', true);
      a.result('stale A interim');
      a.error('not-allowed');
      a.error('no-speech');
      a.end();
      a.started();
    });
    expect(screen.getByRole('textbox')).toHaveValue('B interim');
    expect(screen.getByRole('button', { name: 'Stop recording' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByText(/Microphone access was denied/)).not.toBeInTheDocument();
    expect(b.abort).not.toHaveBeenCalled();
    expect(b.stop).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Stop recording' }));
    expect(b.stop).toHaveBeenCalledTimes(1);
    act(() => b.result('B final', true));
    act(() => b.end());
    expect(screen.getByRole('textbox')).toHaveValue('B final');
    expect(store.getSnapshot('conversation').draft).toBe('B final');
    expect(screen.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
    expect(store.getRejected('conversation')[0]?.text).toBe('@missing.ts A final A interim');
    expect(screen.getByRole('button', { name: 'Restore unsent message' })).toBeDisabled();
    expect(send).toHaveBeenCalledTimes(1);
  });
});
