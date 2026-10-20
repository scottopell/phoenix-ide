import { useEffect, useRef, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConnectedInputArea } from '../components/InputArea';
import { ExpansionError, type FileAttachment, type ImageData } from '../api';
import { DraftContext } from '../conversation/DraftContext';
import { DraftStore } from '../conversation/DraftStore';
import '../index.css';

const original = `  Synthetic draft A\n@missing/${'long-reference-'.repeat(18)}.ts\n  `;
const attachment: FileAttachment = {
  original_name: 'synthetic-recovery-notes.txt', media_type: 'text/plain',
  size_bytes: 42, stored_path: '/synthetic-fixture/recovery-notes.txt',
};

export function DeferredRejection() {
  const [store] = useState(() => {
    const value = new DraftStore();
    value.dispatch('composer-rejection', { type: 'set_draft', text: original });
    return value;
  });
  const [images, setImages] = useState<ImageData[]>([]);
  const [files, setFiles] = useState<FileAttachment[]>([attachment]);
  const rejectPending = useRef<((error: Error) => void) | null>(null);
  const [pending, setPending] = useState(false);
  const [sendCount, setSendCount] = useState(0);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const previous = document.documentElement.dataset['theme'];
    document.documentElement.dataset['theme'] = new URLSearchParams(location.search).get('fixtureTheme') === 'light' ? 'light' : 'dark';
    setReady(true);
    return () => {
      if (previous === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = previous;
    };
  }, []);

  return <MemoryRouter>
    <DraftContext.Provider value={store}>
      <main data-testid="composer-rejection-fixture" data-ready={ready} data-send-count={sendCount}
        style={{ width: '100%', maxWidth: 900, margin: '0 auto', padding: 12, boxSizing: 'border-box' }}>
        <h2>Composer rejection ownership</h2>
        <p>Synthetic text and attachment metadata only. Send A, type B, then reject A. No microphone or upload is used.</p>
        <button type="button" disabled={!pending} style={{ minHeight: 44, marginBottom: 16 }} onClick={() => {
          const reject = rejectPending.current;
          rejectPending.current = null;
          setPending(false);
          reject?.(new ExpansionError({
            error: 'Synthetic expansion rejection: the missing reference cannot be expanded. The original text and attachment must remain available without replacing the newer draft.',
            error_type: 'file_not_found', reference: '@missing/synthetic-reference.ts',
          }));
        }}>Reject pending A</button>
        <ConnectedInputArea slug="composer-rejection" scopeKey="composer-rejection" cwd={undefined}
          convState={{ type: 'idle' }} images={images} setImages={setImages}
          files={files} setFiles={setFiles} isOffline={false} failedMessages={[]}
          onSend={() => {
            setSendCount(count => count + 1);
            setPending(true);
            return new Promise<void>((_resolve, reject) => { rejectPending.current = reject; });
          }} onCancel={() => {}} onRetry={() => {}} />
      </main>
    </DraftContext.Provider>
  </MemoryRouter>;
}
DeferredRejection.storyName = 'deferred rejection';
