import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../../api';
import { AgentMessage } from '../../components/MessageComponents';
import { StreamingMessageView } from '../../components/StreamingMessage';
import { DensityContext } from '../../hooks/useDensity';
import reply from './acl-reply.md?raw';
import './tableFixture.css';

const timestamp = '2026-10-15T12:00:00Z';
const toolResults = new Map<string, Message>();
const tableStart = reply.indexOf('| Path |');
const tableEnd = reply.indexOf('\n\n', tableStart);
const prefixes = [
  { name: 'table-header', end: reply.indexOf('\n', tableStart) },
  { name: 'table-partial-row', end: reply.indexOf('**Blocked**') + 5 },
  { name: 'completed-table-incomplete-tail', end: tableEnd + '\n\nSo the missing permission is '.length },
  { name: 'full-text', end: reply.length },
];

export function StreamingTableFixture() {
  const [prefix, setPrefix] = useState(0);
  const [finalized, setFinalized] = useState(false);
  const [ready, setReady] = useState(false);
  const currentPrefix = prefixes[prefix]!;
  const text = reply.slice(0, currentPrefix.end);
  useEffect(() => {
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setReady(true));
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [prefix, finalized]);
  const message: Message = {
    event_presentation: null, message_id: 'streaming-table', conversation_id: 'table-geometry',
    sequence_id: 1, message_type: 'agent', created_at: timestamp,
    content: [{ type: 'text', text }],
  };
  return (
    <MemoryRouter>
      <DensityContext.Provider value={{ density: 'full', setDensity: () => {} }}>
        <main className="streaming-table-fixture" data-table-ready={ready} data-prefix={currentPrefix.name} data-prefix-length={text.length} data-finalized={finalized}>
          <nav className="streaming-table-controls" aria-label="Table stream controls">
            <button type="button" data-testid="next-prefix" disabled={prefix === prefixes.length - 1 || finalized} onClick={() => { setReady(false); setPrefix(value => value + 1); }}>Next cumulative prefix</button>
            <button type="button" data-testid="finalize-table" disabled={finalized} onClick={() => { setReady(false); setFinalized(true); }}>Finalize identical prefix</button>
            <span>{currentPrefix.name} · {text.length} characters</span>
          </nav>
          <div id="chat-view" className="view active">
            <section className="virtual-transcript-row" data-table-case="subject">
              <h2>{finalized ? 'Finalized subject' : 'Streaming subject'}</h2>
              {finalized
                ? <AgentMessage message={message} toolResults={toolResults} isFirstInTurn forceExpandedText />
                : <StreamingMessageView buffer={{ requestId: 'table-stream', text, lastSequence: prefix, startedAt: Date.parse(timestamp) }} isFirstInTurn />}
            </section>
            <section className="virtual-transcript-row" data-table-case="reference">
              <h2>Finalized reference · identical prefix</h2>
              <AgentMessage message={{ ...message, message_id: 'table-reference' }} toolResults={toolResults} isFirstInTurn forceExpandedText />
            </section>
          </div>
        </main>
      </DensityContext.Provider>
    </MemoryRouter>
  );
}
