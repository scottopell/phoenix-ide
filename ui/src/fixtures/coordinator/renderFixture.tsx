import { useEffect, useMemo, useRef, useState } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import type { Conversation, ImageData } from '../../api';
import { InputArea } from '../../components/InputArea';
import { MessageList } from '../../components/MessageList';
import { StateBar } from '../../components/StateBar';
import { useDocumentViewportOwnership } from '../../components/viewportRoutes';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { CoordinatorPage } from '../../pages/CoordinatorPage';
import { COORDINATOR_QUICK_ACTION } from '../../pages/coordinatorBriefing';
import { getMessageListScenario, messageListFixtureData } from '../messageList';
import '../../index.css';
import type { CoordinatorScenario } from './types';

interface Props {
  scenario: CoordinatorScenario;
}

const coordinatorId = 'fixture-coordinator';

export function CoordinatorFixture({ scenario }: Props) {
  useDocumentViewportOwnership(true);
  const store = useMemo(() => new ConversationStore(), []);

  useEffect(() => {
    document.documentElement.dataset['theme'] = 'dark';
    document.documentElement.dataset['coordinatorFixtureReady'] = scenario.id;
    return () => { delete document.documentElement.dataset['coordinatorFixtureReady']; };
  }, [scenario]);

  return (
    <ConversationContext.Provider value={store}>
      <MemoryRouter initialEntries={[`/global/${coordinatorId}`]}>
        <Routes>
        <Route
          path="/global/:slug"
          element={(
            <CoordinatorPage
              fixtureData={{
                coordinatorId,
                conversation: <FixtureConversation scenario={scenario} />,
              }}
            />
          )}
        />
        </Routes>
      </MemoryRouter>
    </ConversationContext.Provider>
  );
}

function FixtureConversation({ scenario }: { scenario: CoordinatorScenario }) {
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [fixtureConnectionState, setFixtureConnectionState] = useState(scenario.connectionState);
  const fixturePhaseStartedAt = useMemo(() => Date.now() - 12_000, []);
  const fixtureLastEventAtRef = useRef(Date.now() - (scenario.staleWatchdog ? 36_000 : 0));
  const store = useMemo(() => new ConversationStore(), []);
  const transcript = useMemo(
    () => messageListFixtureData(getMessageListScenario('compact-latest-expanded')),
    [],
  );
  const convState = scenario.working ? { type: 'llm_requesting', attempt: 1 } as const : { type: 'idle' } as const;
  useEffect(() => {
    if (!scenario.freezeReconnectAfterMount) return;
    const timer = window.setTimeout(() => setFixtureConnectionState('reconnecting'), 0);
    return () => window.clearTimeout(timer);
  }, [scenario.freezeReconnectAfterMount]);
  const conversation: Conversation = {
    id: transcript.conversationId,
    slug: transcript.slug,
    model: 'claude-sonnet-5',
    cwd: '/work/phoenix',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    message_count: transcript.messages.length,
    state: convState,
    branch_name: null,
    base_branch: null,
    worktree_path: null,
    task_title: null,
    conv_mode_label: 'Explore',
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'global:',
  };

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: 'compact', setDensity: () => {} }}>
        <div id="app">
          <div className="conversation-column">
            <MessageList
              messages={transcript.messages}
              pendingMessages={[]}
              convState={convState}
              onRetry={() => {}}
              onOpenFile={() => {}}
              conversationId={transcript.conversationId}
              slug={transcript.slug}
              transcriptPositioning={{ kind: 'idle', view: { conversationId: transcript.conversationId, generation: 1, transcriptGeneration: 1 } }}
            />
            <InputArea
              cwd={undefined}
              scopeKey={transcript.conversationId}
              convState={convState}
              images={images}
              setImages={setImages}
              isOffline={fixtureConnectionState !== 'connected'}
              failedMessages={[]}
              convModeLabel="Explore"
              draft={draft}
              onDraftChange={setDraft}
              quickAction={COORDINATOR_QUICK_ACTION}
              onSend={() => {}}
              onCancel={() => {}}
              onRetry={() => {}}
            />
            <StateBar
              conversation={conversation}
              convState={convState}
              connectionState={fixtureConnectionState}
              connectionAttempt={fixtureConnectionState === 'reconnecting' ? 12 : 0}
              nextRetryIn={fixtureConnectionState === 'offline' ? 4 : null}
              contextWindowUsed={16_000}
              modelContextWindow={200_000}
              phaseStateUpdatedAt={scenario.working ? fixturePhaseStartedAt : null}
              lastSseEventAtRef={fixtureLastEventAtRef}
              conversationExtension={scenario.globalActivity ? {
                summary: (
                  <span className="global-statebar-summary">
                    <span>Watching 3</span>
                    <span>Running 1</span>
                    <span className="global-statebar-summary__auto">Auto On · Continuing</span>
                  </span>
                ),
                details: (
                  <div className="global-statebar-activity__details">
                    <section className="global-active-watches" aria-label="Active watches">
                      <strong>Watching</strong>
                      {['Review release readiness', 'Fix mobile transcript', 'Verify reconnect recovery'].map((title) => (
                        <div className="global-active-watch" key={title}>
                          <div><a href="#fixture-watch">{title}</a><span>Working</span></div>
                          <a href="#fixture-transcript">current transcript</a>
                          <details className="global-activity-metadata"><summary>Details</summary></details>
                        </div>
                      ))}
                    </section>
                    <section className="global-live-commands" aria-label="Running commands">
                      <strong>Running</strong>
                      <div className="global-live-command">
                        <div><strong>Global visual QA</strong><code>pnpm test</code><span>Started just now</span></div>
                        <div className="global-live-command-actions"><a href="#fixture-output">output →</a></div>
                      </div>
                    </section>
                  </div>
                ),
              } : undefined}
            />
          </div>
        </div>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
