import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { MessageList, type MessageListHandle } from '../../components/MessageList';
import type { HistoryScrollCommand } from '../../conversation/historyExpansion';
import type { TranscriptPositioningInput } from '../../conversation/transcriptPositioning';
import '../../index.css';
import type { Message } from '../../api';
import type { MessageListScenario } from './types';
import {
  compactChronologyAppendMessages,
  compactChronologyCompletionMessages,
  compactChronologyFinalMessages,
  messageListFixtureData,
  prefixContinuityEarlierMessages,
  mobileTablePreviewMarkdown,
} from './scenarios';
import { StreamingBlocks } from '../../components/StreamingMessage';
import './mobileTablePreview.css';

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
    __messageListChronologyMetrics?: unknown;
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
  const isChronologyScenario = scenario.id === 'compact-expanded-tool-chronology';
  const isMobileTablePreview = scenario.id.startsWith('mobile-table-preview-');
  const [mobilePreviewState, setMobilePreviewState] = useState<'final' | 'streaming'>('streaming');
  const mobileStatusJumpPendingRef = useRef(true);

  const recordChronologyMetrics = useCallback((phase: string) => {
    const scroller = document.querySelector<HTMLElement>('.message-list-fixture-shell #messages');
    const toolNodes = Array.from(document.querySelectorAll<HTMLElement>('[data-tool-id]'));
    const latest = phase === 'final-prose'
      ? document.querySelector<HTMLElement>('#message-chronology-agent-final')
      : ['chronology-agent-bc', 'chronology-agent-a']
        .map((id) => document.querySelector<HTMLElement>(`[data-message-id="${id}"]`))
        .find((node): node is HTMLElement => Boolean(node));
    const expanded = document.querySelector<HTMLElement>('[data-tool-id="chronology-tool-a"]');
    const scrollerRect = scroller?.getBoundingClientRect();
    const payload = {
      phase,
      renderUnitOrder: Array.from(document.querySelectorAll<HTMLElement>('[data-render-unit-key]')).map((node) => node.dataset['renderUnitKey']),
      toolDomOrder: toolNodes.map((node) => node.dataset['toolId']),
      expandedA: expanded && scrollerRect ? {
        top: expanded.getBoundingClientRect().top - scrollerRect.top,
        bottom: expanded.getBoundingClientRect().bottom - scrollerRect.top,
      } : null,
      scroll: scroller ? {
        scrollTop: scroller.scrollTop,
        scrollHeight: scroller.scrollHeight,
        clientHeight: scroller.clientHeight,
        atTail: Math.abs(scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop) <= 4,
      } : null,
      latestReachable: Boolean(latest && scroller && latest.offsetTop <= scroller.scrollHeight - latest.offsetHeight),
      expandedCount: document.querySelectorAll('.compact-tool-selected-detail').length,
    };
    window.__messageListChronologyMetrics = payload;
    document.documentElement.dataset['messageListChronologyPhase'] = phase;
  }, []);

  const measureAnchor = useCallback((messageId: string) => {
    const scroller = document.querySelector<HTMLElement>('.message-list-fixture-shell #messages');
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
      if (!isMobileTablePreview) {
        document.documentElement.dataset['messageListFixtureReady'] = scenario.id;
      }
    }, 50);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      delete document.documentElement.dataset['messageListFixtureReady'];
    };
  }, [data.theme, isMobileTablePreview, scenario.id]);

  useEffect(() => {
    setMessages(data.messages);
  }, [data.messages]);

  const scrollFinalStatusTable = useCallback(() => {
    const viewport = document.querySelector<HTMLElement>(
      '.mobile-table-preview .fixture-message-list-stage .virtual-transcript.message-virtual-transcript',
    );
    if (!viewport || viewport.clientHeight <= 200) return false;
    const table = [...viewport.querySelectorAll('table')].find((candidate) => {
      const headers = [...candidate.querySelectorAll('th')].map(
        (header) => header.textContent?.trim(),
      );
      return headers[0] === 'Stream' && headers[1] === 'Current position';
    });
    if (!table) return false;
    const viewportRect = viewport.getBoundingClientRect();
    const tableRect = table.getBoundingClientRect();
    viewport.scrollTo({
      top: viewport.scrollTop + tableRect.top - viewportRect.top,
      behavior: 'auto',
    });
    mobileStatusJumpPendingRef.current = false;
    return true;
  }, []);

  const jumpToMobileStatus = useCallback(() => {
    mobileStatusJumpPendingRef.current = true;
    if (mobilePreviewState === 'final' && scrollFinalStatusTable()) return;
    setMobilePreviewState('final');
  }, [mobilePreviewState, scrollFinalStatusTable]);

  useLayoutEffect(() => {
    if (!isMobileTablePreview || !ready) return;
    delete document.documentElement.dataset['messageListFixtureReady'];
    const stage = document.querySelector<HTMLElement>('.mobile-table-preview .fixture-message-list-stage');
    if (!stage) return;

    const findStatusTable = (root: ParentNode) => [...root.querySelectorAll('table')].find((candidate) => {
      const headers = [...candidate.querySelectorAll('th')].map(
        (header) => header.textContent?.trim(),
      );
      return headers[0] === 'Stream' && headers[1] === 'Current position';
    });
    const verify = () => {
      const viewport = mobilePreviewState === 'streaming'
        ? stage
        : stage.querySelector<HTMLElement>('.virtual-transcript.message-virtual-transcript');
      if (!viewport || viewport.clientHeight <= 200) return;
      const table = findStatusTable(viewport);
      if (!table) return;
      if (mobilePreviewState === 'final' && mobileStatusJumpPendingRef.current) {
        scrollFinalStatusTable();
      }
      const viewportRect = viewport.getBoundingClientRect();
      const tableRect = table.getBoundingClientRect();
      if (tableRect.bottom > viewportRect.top && tableRect.top < viewportRect.bottom) {
        document.documentElement.dataset['messageListFixtureViewportHeight'] = String(viewport.clientHeight);
        document.documentElement.dataset['messageListFixtureStatusTableTop'] = String(
          Math.round(tableRect.top - viewportRect.top),
        );
        document.documentElement.dataset['messageListFixtureReady'] = scenario.id;
      }
    };

    const resizeObserver = new ResizeObserver(verify);
    resizeObserver.observe(stage);
    const mutationObserver = new MutationObserver(verify);
    mutationObserver.observe(stage, { childList: true, subtree: true });
    stage.addEventListener('scroll', verify, true);
    verify();
    return () => {
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      stage.removeEventListener('scroll', verify, true);
      delete document.documentElement.dataset['messageListFixtureReady'];
      delete document.documentElement.dataset['messageListFixtureViewportHeight'];
      delete document.documentElement.dataset['messageListFixtureStatusTableTop'];
    };
  }, [isMobileTablePreview, mobilePreviewState, ready, scenario.id, scrollFinalStatusTable]);

  const reproduceContinuityJump = () => {
    const messageList = messageListRef.current;
    if (!messageList) return;
    const basis = messageList.captureHistoryRestoreBasis();
    if (!basis || basis.kind !== 'reader_anchor') return;
    const measured = measureAnchor(basis.messageId);
    if (!measured) return;
    pendingAnchorRef.current = { messageId: basis.messageId, offset: basis.viewportStartOffset };
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
      messageId: pending.messageId,
      viewportStartOffset: pending.offset,
    });
  }, [data.conversationId, messages]);

  const transcriptPositioning: TranscriptPositioningInput = historyScrollCommand
    ? { kind: 'positioning', command: historyScrollCommand }
    : { kind: 'idle', view: { conversationId: data.conversationId, generation: 1, transcriptGeneration: 1 } };

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

  const appendChronologyTools = () => {
    setMessages((current) => [...current, ...compactChronologyAppendMessages]);
    requestAnimationFrame(() => recordChronologyMetrics('appended-bc'));
  };

  const completeChronologyTools = () => {
    setMessages((current) => (
      current.some((message) => message.message_id === 'chronology-result-b')
        ? current
        : [...current, ...compactChronologyCompletionMessages]
    ));
    requestAnimationFrame(() => recordChronologyMetrics('completed-bc'));
  };

  const appendChronologyFinal = () => {
    setMessages((current) => [...current, ...compactChronologyFinalMessages]);
    requestAnimationFrame(() => recordChronologyMetrics('final-prose'));
  };

  const jumpChronologyLatest = () => {
    const scroller = document.querySelector<HTMLElement>('.message-list-fixture-shell #messages');
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
    requestAnimationFrame(() => recordChronologyMetrics('jump-latest'));
  };

  if (!ready) return null;

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: 'compact', setDensity: () => {} }}>
        <MemoryRouter initialEntries={[`/c/${data.slug}`]}>
          <main
            className={`fixture-page ${isMobileTablePreview ? 'mobile-table-preview' : ''}`}
            data-message-list-fixture={scenario.id}
          >
            <div className="fixture-toolbar">
              <strong>Message list fixture</strong>
              <span>scenario={scenario.id}</span>
              <span>density=compact</span>
              {isMobileTablePreview && <span>deployed table CSS=838439341 · fixture source=7a42b66db</span>}
              {isMobileTablePreview && (
                <nav className="mobile-table-preview-controls" aria-label="Preview controls">
                  <button
                    type="button"
                    aria-pressed={mobilePreviewState === 'final'}
                    onClick={jumpToMobileStatus}
                  >
                    Final
                  </button>
                  <button
                    type="button"
                    aria-pressed={mobilePreviewState === 'streaming'}
                    onClick={() => setMobilePreviewState('streaming')}
                  >
                    Streaming
                  </button>
                  <button type="button" onClick={jumpToMobileStatus}>
                    Status
                  </button>
                  <a href="?story=message-list--mobile-table-preview-baseline">Baseline</a>
                  <a href="?story=message-list--mobile-table-preview-content-wrap">Wrap</a>
                  <a href="?story=message-list--mobile-table-preview-readable-overflow">Overflow</a>
                </nav>
              )}
              {scenario.id === 'scroll-policy-long' && (
                <button type="button" data-testid="append-tail" onClick={appendTail}>
                  Append tail
                </button>
              )}
              {isChronologyScenario && (
                <>
                  <button type="button" data-testid="chronology-measure" onClick={() => recordChronologyMetrics('manual')}>
                    Measure chronology
                  </button>
                  <button type="button" data-testid="chronology-append-bc" onClick={appendChronologyTools}>
                    Append B/C
                  </button>
                  <button type="button" data-testid="chronology-complete-bc" onClick={completeChronologyTools}>
                    Complete B/C
                  </button>
                  <button type="button" data-testid="chronology-final" onClick={appendChronologyFinal}>
                    Append final prose
                  </button>
                  <button type="button" data-testid="chronology-jump-latest" onClick={jumpChronologyLatest}>
                    Jump latest
                  </button>
                </>
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
              {isMobileTablePreview && mobilePreviewState === 'streaming' ? (
                <section className="mobile-table-preview-streaming" aria-label="Streaming state">
                  <strong>Streaming state — same production Markdown renderer</strong>
                  <div className="message agent">
                    <div className="message-content">
                      <div className="agent-text-block streaming">
                        <StreamingBlocks text={mobileTablePreviewMarkdown} />
                      </div>
                    </div>
                  </div>
                </section>
              ) : (
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
                    transcriptPositioning={transcriptPositioning}
                    onHistoryScrollCommandHandled={handleHistoryCommand}
                  />
                </div>
              )}
            </div>
          </main>
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
