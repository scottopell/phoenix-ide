import { useEffect, useState } from 'react';
import type { TranscriptPositioningInput } from '../../conversation/transcriptPositioning';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../../api';
import { MessageList } from '../../components/MessageList';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { useConversationView } from '../../conversation/useConversationAtom';
import { DensityContext, type Density } from '../../hooks/useDensity';
import './transcriptFixture.css';

const slug = 'finalization-transcript';
const requestId = 'finalization-response';
const timestamp = '2026-10-15T12:00:00Z';
const paragraphs = Array.from({ length: 24 }, (_, index) =>
  `Reading marker ${String(index + 1).padStart(2, '0')}. This paragraph keeps identical text through token delivery and finalization. The reader owns the viewport while inspecting this response; the appearance of Review and Copy must not move these words or pull the reader to the bottom.`);
const text = paragraphs.join('\n\n');
const user: Message = {
  event_presentation: null, message_id: 'finalization-user', conversation_id: slug,
  sequence_id: 1, message_type: 'user', created_at: timestamp,
  content: { text: 'Explain the result in detail so I can read it before streaming finishes.' },
};

const positioning: TranscriptPositioningInput = {
  kind: 'idle', view: { conversationId: slug, generation: 0, transcriptGeneration: 0 },
};

function createStore() {
  const store = new ConversationStore();
  store.dispatch(slug, {
    type: 'set_initial_data', conversationId: slug,
    conversation: {
      id: slug, slug, model: 'fixture-model', cwd: '/fixture',
      created_at: timestamp, updated_at: timestamp, message_count: 1,
      browser_session_active: false, terminal_uses_tmux: false,
      work_scope_key: `conversation:${slug}`, state: { type: 'idle' },
    },
    messages: [user], phase: { type: 'idle' }, contextWindow: { used: 0 },
    transcriptGeneration: 0, transcriptCoverage: 'complete', eventCursorFloor: 1,
  });
  store.dispatch(slug, {
    type: 'sse_state_change', sequenceId: 2,
    phase: { type: 'llm_requesting', attempt: 1 }, stateUpdatedAt: Date.parse(timestamp),
  });
  store.dispatch(slug, { type: 'sse_token', sequenceId: 3, requestId, delta: text });
  return store;
}

function Transcript({ retry }: { retry: boolean }) {
  const [view, dispatch] = useConversationView(slug);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let frame = 0;
    void document.fonts.ready.then(() => {
      if (cancelled) return;
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setReady(true));
      });
    });
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, []);
  const finalized = view.messages.some(message => message.message_id === requestId);
  const finalize = () => {
    dispatch({
      type: 'sse_message', sequenceId: 4,
      message: {
        event_presentation: null, message_id: requestId, conversation_id: slug,
        sequence_id: 2, message_type: 'agent', created_at: timestamp,
        content: [{ type: 'text', text }], display_data: retry ? { retry_count: 2 } : null,
      },
    });
    dispatch({ type: 'sse_state_change', sequenceId: 5, phase: { type: 'idle' }, stateUpdatedAt: Date.parse(timestamp) });
  };
  return (
    <main className="finalization-transcript-fixture" data-transcript-ready={ready} data-transcript-finalized={finalized} data-transcript-retry={retry} data-streaming-request-id={view.streamingRequestId ?? ''}>
      <div className="finalization-transcript-controls">
        <button type="button" data-testid="finalize-transcript" disabled={finalized} onClick={finalize}>Finalize identical text</button>
      </div>
      <div className="finalization-transcript-shell">
        <MessageList
          enableMessageSidepanel transcriptPositioning={positioning}
          messages={view.messages} streamingRequestId={view.streamingRequestId}
          pendingMessages={[]} convState={view.phase} conversationId={slug} slug={slug}
          onRetry={() => {}} onOpenFile={() => {}}
        />
      </div>
    </main>
  );
}

export function AssistantFinalizationTranscriptFixture({ density, retry = false }: { density: Density; retry?: boolean }) {
  const [store] = useState(createStore);
  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density, setDensity: () => {} }}>
        <MemoryRouter initialEntries={[`/c/${slug}`]}>
          <Transcript retry={retry} />
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
