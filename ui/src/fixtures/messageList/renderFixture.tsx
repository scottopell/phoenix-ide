import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { MessageList, type MessageListHandle } from '../../components/MessageList';
import type { HistoryScrollCommand } from '../../conversation/historyExpansion';
import '../../index.css';
import type { Message } from '../../api';
import type { MessageListScenario } from './types';
import { messageListFixtureData, prefixContinuityEarlierMessages } from './scenarios';

interface Props {
  scenario: MessageListScenario;
}

type ContinuityMilestone = {
  name: 'before-prefix' | 'after-restore';
  anchorMessageId: string;
  anchorOffset: number;
  scrollTop: number;
  drift?: number;
};

declare global {
  interface Window {
    __messageListContinuityTrace?: ContinuityMilestone[];
  }
}

export function MessageListFixture({ scenario }: Props) {
  const [ready, setReady] = useState(false);
  const data = useMemo(() => messageListFixtureData(scenario), [scenario]);
  const [messages, setMessages] = useState<Message[]>(data.messages);
  const store = useMemo(() => new ConversationStore(), []);
  const messageListRef = useRef<MessageListHandle>(null);
  const [historyScrollCommand, setHistoryScrollCommand] = useState<HistoryScrollCommand | null>(null);
  const pendingAnchorRef = useRef<{ messageId: string; offset: number } | null>(null);
  const [continuityTrace, setContinuityTrace] = useState<ContinuityMilestone[]>([]);
  const isContinuityScenario = scenario.id === 'prefix-continuity-offset-bug';

  const measureAnchor = useCallback((messageId: string) => {
    const scroller = document.querySelector<HTMLElement>('.message-list-fixture-shell [data-testid="virtuoso-scroller"]')
      ?? document.querySelector<HTMLElement>('.message-list-fixture-shell [data-virtuoso-scroller="true"]');
    const marker = Array.from(document.querySelectorAll<HTMLElement>('[data-render-unit-key]'))
      .find((row) => row.textContent?.includes('Continuity marker 01'));
    if (!scroller || !marker) return null;
    return {
      anchorMessageId: messageId,
      anchorOffset: marker.getBoundingClientRect().top - scroller.getBoundingClientRect().top,
      scrollTop: scroller.scrollTop,
    };
  }, []);

  const recordMilestone = useCallback((milestone: ContinuityMilestone) => {
    setContinuityTrace((current) => {
      const next = [...current, milestone];
      window.__messageListContinuityTrace = next;
      document.documentElement.dataset['messageListContinuityMilestone'] = milestone.name;
      return next;
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    delete document.documentElement.dataset['messageListFixtureReady'];
    document.documentElement.dataset['theme'] = data.theme;
    const timer = window.setTimeout(() => {
      if (cancelled) return;
      setReady(true);
      document.documentElement.dataset['messageListFixtureReady'] = scenario.id;
    }, 50);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      delete document.documentElement.dataset['messageListFixtureReady'];
    };
  }, [data.theme, scenario.id]);

  useEffect(() => {
    setMessages(data.messages);
  }, [data.messages]);

  const reproduceContinuityJump = () => {
    const basis = messageListRef.current?.captureHistoryRestoreBasis();
    if (!basis || basis.kind !== 'reader_anchor') return;
    const measured = measureAnchor(basis.messageId);
    if (!measured) return;
    pendingAnchorRef.current = { messageId: basis.messageId, offset: measured.anchorOffset };
    recordMilestone({ name: 'before-prefix', ...measured });
    setMessages((current) => [...prefixContinuityEarlierMessages, ...current]);
  };

  useLayoutEffect(() => {
    const pending = pendingAnchorRef.current;
    if (!pending || messages[0]?.message_id !== prefixContinuityEarlierMessages[0]?.message_id) return;
    setHistoryScrollCommand({
      kind: 'restore_after_prefix_expansion',
      token: 1,
      requestToken: 1,
      view: { conversationId: data.conversationId, generation: 1, transcriptGeneration: 1 },
      anchorMessageId: pending.messageId,
    });
  }, [data.conversationId, messages]);

  const handleHistoryCommand = (token: number) => {
    if (token !== 1) return;
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const pending = pendingAnchorRef.current;
      if (!pending) return;
      const measured = measureAnchor(pending.messageId);
      if (!measured) return;
      recordMilestone({
        name: 'after-restore',
        ...measured,
        drift: measured.anchorOffset - pending.offset,
      });
      pendingAnchorRef.current = null;
    }));
  };

  const appendTail = () => {
    setMessages((current) => {
      const sequenceId = current.length + 1;
      return [...current, {
        message_id: `fixture-tail-${sequenceId}`,
        conversation_id: data.conversationId,
        sequence_id: sequenceId,
        type: 'user',
        message_type: 'user',
        created_at: new Date().toISOString(),
        content: { text: `Appended tail item ${sequenceId}` },
        display_data: {},
      }];
    });
  };

  if (!ready) return null;

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: 'compact', setDensity: () => {} }}>
        <MemoryRouter initialEntries={[`/c/${data.slug}`]}>
          <main className="fixture-page" data-message-list-fixture={scenario.id}>
            <div className="fixture-toolbar">
              <strong>Message list fixture</strong>
              <span>scenario={scenario.id}</span>
              <span>density=compact</span>
              {scenario.id === 'scroll-policy-long' && (
                <button type="button" data-testid="append-tail" onClick={appendTail}>
                  Append tail
                </button>
              )}
              {isContinuityScenario && (
                <button type="button" data-testid="reproduce-prefix-jump" onClick={reproduceContinuityJump}>
                  Load earlier history
                </button>
              )}
              {isContinuityScenario && continuityTrace.map((milestone) => (
                <span key={milestone.name} data-continuity-milestone={milestone.name}>
                  {milestone.name}: offset={milestone.anchorOffset.toFixed(1)}
                  {milestone.drift === undefined ? '' : ` drift=${milestone.drift.toFixed(1)}`}
                </span>
              ))}
            </div>
            <div className="fixture-message-list-stage">
              <div className="message-list-fixture-shell">
                <MessageList
                  ref={messageListRef}
                  messages={messages}
                  pendingMessages={data.pendingMessages}
                  convState={data.convState}
                  onRetry={() => {}}
                  onOpenFile={() => {}}
                  conversationId={data.conversationId}
                  slug={data.slug}
                  historyScrollCommand={historyScrollCommand}
                  onHistoryScrollCommandHandled={handleHistoryCommand}
                />
              </div>
            </div>
          </main>
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
