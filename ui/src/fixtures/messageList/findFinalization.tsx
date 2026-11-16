import { useEffect, useMemo, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../../api';
import { MessageList } from '../../components/MessageList';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { FocusScopeProvider } from '../../hooks/useFocusScope';
import '../../index.css';

const text = ['alpha identical final text', ...Array.from({length: 35}, (_, i) => `Reading marker ${i}: This paragraph stays identical when streaming becomes a finalized message.`)].join('\n\n');
const slug = 'fixture-find-finalization';
const requestId = 'find-finalization-request';

function message(id: string, sequence: number, body: string): Message {
  return {
    message_id: id,
    sequence_id: sequence,
    conversation_id: slug,
    message_type: 'agent',
    content: [{ type: 'text', text: body }],
    created_at: '2026-01-01T00:00:00Z',
    event_presentation: null,
  };
}

const history = Array.from({ length: 12 }, (_, index) => message(
  `find-history-${index}`, index + 1,
  index === 11 ? 'alpha historical match' : `Historical paragraph ${index}.\n\nOrdinary transcript content.`,
));

declare global {
  interface Window {
    __messageListFinalizeFind?: () => void;
  }
}

export function FindFinalizationFixture() {
  const [finalized, setFinalized] = useState(false);
  const store = useMemo(() => {
    const conversation = new ConversationStore();
    conversation.dispatch(slug, {
      type: 'sse_state_change', sequenceId: 1,
      phase: { type: 'llm_requesting', attempt: 1 }, stateUpdatedAt: Date.now(),
    });
    conversation.dispatch(slug, {
      type: 'sse_token', sequenceId: 2, requestId, delta: text,
    });
    return conversation;
  }, []);

  useEffect(() => {
    window.__messageListFinalizeFind = () => setFinalized(true);
    return () => { delete window.__messageListFinalizeFind; };
  }, []);

  return (
    <ConversationContext.Provider value={store}>
      <MemoryRouter>
        <FocusScopeProvider>
          <div
            className="message-list-fixture-shell"
            style={{ height: 600, width: 760, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}
            data-message-list-fixture-ready="find-identical-finalization"
            data-find-finalization={finalized ? 'final' : 'streaming'}
          >
            <MessageList
              slug={slug}
              conversationId={slug}
              messages={finalized ? [...history, message(requestId, 13, text)] : history}
              pendingMessages={[]}
              streamingRequestId={finalized ? null : requestId}
              convState={finalized ? { type: 'idle' } : { type: 'llm_requesting', attempt: 1 }}
              onRetry={() => {}}
              onOpenFile={undefined}
              transcriptPositioning={{ kind: 'idle', view: { conversationId: slug, generation: 1, transcriptGeneration: 1 } }}
            />
          </div>
        </FocusScopeProvider>
      </MemoryRouter>
    </ConversationContext.Provider>
  );
}
