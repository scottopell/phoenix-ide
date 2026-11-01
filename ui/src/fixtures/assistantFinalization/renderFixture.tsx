import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../../api';
import { AgentMessage, UserMessage } from '../../components/MessageComponents';
import { StreamingMessageView } from '../../components/StreamingMessage';
import { MessageReviewEnabledContext } from '../../components/MessageReviewAction';
import { DensityContext, type Density } from '../../hooks/useDensity';
import './renderFixture.css';

const timestamp = '2026-10-15T12:00:00Z';
const text = 'The assistant response keeps the same prose width while streaming and after finalization. Review and Copy belong in their own toolbar, not in a gutter taken from this paragraph. A narrow phone should wrap these words identically in both states.\n\nThis second paragraph checks the downstream geometry as well as the first line. Finalization must not insert an action row, change the text inset, or move the next message. The same checks apply to the first response in a turn and to a continuation after a tool result.';
const toolResults = new Map<string, Message>();
const modes = ['review', 'copy-only', 'suppressed', 'handoff'] as const;
const cases = [true, false].flatMap(isFirstInTurn => modes.map(mode => ({
  id: `${isFirstInTurn ? 'first' : 'continuation'}-${mode}`, isFirstInTurn, mode,
})));

export function AssistantFinalizationFixture({ density }: { density: Density }) {
  const [finalized, setFinalized] = useState(false);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => setReady(true));
    });
    return () => cancelAnimationFrame(frame);
  }, []);
  return (
    <MemoryRouter>
      <DensityContext.Provider value={{ density, setDensity: () => {} }}>
        <main className="assistant-finalization-fixture" data-finalization-ready={ready} data-finalized={finalized} data-column={new URLSearchParams(window.location.search).get('column') === 'narrow' ? 'narrow' : 'normal'}>
          <button type="button" data-testid="finalize-assistant" onClick={() => setFinalized(true)}>Finalize identical text</button>
          <UserMessage message={{ origin: { kind: 'user_api' }, event_presentation: null, message_id: 'user-control', conversation_id: 'geometry', sequence_id: 1, message_type: 'user', created_at: timestamp, content: { text: 'User message geometry is outside the assistant chrome change.' } }} />
          {cases.map(({ id, isFirstInTurn, mode }, index) => {
            const message: Message = {
              event_presentation: null, message_id: id, conversation_id: 'geometry', sequence_id: index + 2,
              message_type: 'agent', created_at: timestamp, content: [{ type: 'text', text }],
              display_data: mode === 'handoff' ? { productHistoricalHandoff: true } : null,
            };
            return (
              <section key={id} data-geometry-case={id}>
                <h2>{id}</h2>
                <MessageReviewEnabledContext.Provider value={mode !== 'copy-only'}>
                  {finalized
                    ? <AgentMessage message={message} toolResults={toolResults} isFirstInTurn={isFirstInTurn} suppressMessageCopy={mode === 'suppressed'} />
                    : <StreamingMessageView buffer={{ requestId: id, text, lastSequence: 1, startedAt: Date.parse(timestamp) }} isFirstInTurn={isFirstInTurn} />}
                </MessageReviewEnabledContext.Provider>
                <div data-geometry-downstream>Following row</div>
              </section>
            );
          })}
        </main>
      </DensityContext.Provider>
    </MemoryRouter>
  );
}
