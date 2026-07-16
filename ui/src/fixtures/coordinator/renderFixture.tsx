import { useEffect, useMemo, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { useMediaQuery } from '../../hooks';
import { MessageList } from '../../components/MessageList';
import { getMessageListScenario, messageListFixtureData } from '../messageList';
import '../../index.css';
import '../../pages/CoordinatorPage.css';
import type { CoordinatorScenario } from './types';

interface Props {
  scenario: CoordinatorScenario;
}

export function CoordinatorFixture({ scenario }: Props) {
  const [view, setView] = useState<'conversation' | 'fleet'>(scenario.initialView);
  const compactLayout = useMediaQuery('(max-width: 1024px)');
  const store = useMemo(() => new ConversationStore(), []);
  const transcript = useMemo(
    () => messageListFixtureData(getMessageListScenario('compact-latest-expanded')),
    [],
  );

  useEffect(() => {
    document.documentElement.dataset['theme'] = 'dark';
    document.documentElement.dataset['coordinatorFixtureReady'] = scenario.id;
    setView(scenario.initialView);
    return () => { delete document.documentElement.dataset['coordinatorFixtureReady']; };
  }, [scenario]);

  return (
    <MemoryRouter>
      <ConversationContext.Provider value={store}>
        <DensityContext.Provider value={{ density: 'compact', setDensity: () => {} }}>
          <main className={`coordinator-page coordinator-page--${view}`} data-coordinator-fixture={scenario.id} style={{ height: '100dvh' }}>
            <header className="coordinator-header">
              <button type="button" className="coordinator-back" aria-label="Back">←</button>
              <div className="coordinator-heading"><h1>Coordinator</h1><p>Durable fleet coordination.</p></div>
              <div className="coordinator-actions"><button type="button">Refresh fleet</button></div>
              <div className="coordinator-view-switch" role="group" aria-label="Coordinator view">
                <button type="button" aria-pressed={view === 'conversation'} onClick={() => setView('conversation')}>Conversation</button>
                <button type="button" aria-pressed={view === 'fleet'} onClick={() => setView('fleet')}>Fleet <span className="coordinator-view-count">2</span></button>
              </div>
            </header>

            <section className="coordinator-conversation" aria-label="Coordinator conversation" hidden={compactLayout && view !== 'conversation'}>
              <div id="app">
                <div className="conversation-column">
                  <MessageList
                    messages={transcript.messages}
                    pendingMessages={[]}
                    convState={scenario.working ? { type: 'llm_requesting', attempt: 1 } : { type: 'idle' }}
                    onRetry={() => {}}
                    onOpenFile={() => {}}
                    conversationId={transcript.conversationId}
                    slug={transcript.slug}
                    transcriptPositioning={{ kind: 'idle', view: { conversationId: transcript.conversationId, generation: 1, transcriptGeneration: 1 } }}
                  />
                  {scenario.working && <div className="continuation-progress"><div className="continuation-progress-header">Agent working…</div><div className="continuation-progress-text">You can queue a follow-up.</div></div>}
                  <div id="input-area"><textarea aria-label="Message Coordinator" defaultValue="" placeholder="Ask about work across Phoenix…" /></div>
                  <div id="state-bar" className="statebar-mobile"><span>coordinator</span><span>{scenario.working ? 'awaiting LLM response' : 'ready'}</span></div>
                </div>
              </div>
            </section>

            <div className="coordinator-fleet-pane" hidden={compactLayout && view !== 'fleet'}>
              <section className="coordinator-open-work">
                <div className="coordinator-section-title"><h2>Fleet</h2><span>2 items</span></div>
                {scenario.fleetError ? <div className="coordinator-error">Fleet unavailable: projection unavailable</div> : (
                  <section className="coordinator-project">
                    <div className="coordinator-project-header"><div><h3>Phoenix</h3><div className="coordinator-path">/work/phoenix</div></div><span>2</span></div>
                    <div className="coordinator-items">
                      <article className="coordinator-item">
                        <div className="coordinator-item-row">
                          <div className="coordinator-item-main"><div className="coordinator-item-title-row"><a href="/c/mobile-first-coordinator">Restore mobile Coordinator transcript</a><span className="coordinator-state-pill">working</span></div><div className="coordinator-compact-meta"><span>chain</span><span>WORK</span><span>4m</span><span>TASK 44006</span></div></div>
                          <div className="coordinator-item-actions"><button type="button">{scenario.expanded ? 'Hide details' : 'Show details'}</button><button type="button">Copy ref</button></div>
                        </div>
                        {scenario.expanded && <div className="coordinator-item-details"><div className="coordinator-signals"><span>active runtime</span><span>task open</span></div><div className="coordinator-work-meta"><span>CURRENT a27dd240</span><span>ROOT 86f2ce14</span><span>WORKTREE /phoenix/worktrees/mobile-first-coordinator</span><span>REF @work:mobile-first-coordinator</span></div></div>}
                      </article>
                    </div>
                  </section>
                )}
              </section>
            </div>
          </main>
        </DensityContext.Provider>
      </ConversationContext.Provider>
    </MemoryRouter>
  );
}
