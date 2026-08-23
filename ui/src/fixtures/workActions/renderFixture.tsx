import { useEffect } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ThemeContext } from '../../hooks/useTheme';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { WorkControlBar } from '../../components/WorkActions';
import type { WorkActionsScenario } from './types';
import '../../index.css';
import './renderFixture.css';

interface Props {
  scenario: WorkActionsScenario;
}

const noop = () => {};

export function WorkActionsFixture({ scenario }: Props) {
  useEffect(() => {
    const previousTheme = document.documentElement.dataset['theme'];
    document.documentElement.dataset['theme'] = 'dark';
    return () => {
      if (previousTheme === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, []);

  const handle = {
    state: scenario.prState,
    refresh: async () => undefined,
    refreshForSafety: async () => undefined,
    refreshAfterMutation: async () => undefined,
    activeSelection: null,
    activePrSummary: null,
    ambiguous: false,
    pinActivePr: async () => undefined,
    resumeInference: async () => undefined,
  };

  return (
    <ThemeContext.Provider value={{ theme: 'dark', toggleTheme: noop }}>
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <main id="app" className="work-actions-conversation-fixture" data-work-actions-fixture={scenario.id}>
            <header className="work-actions-conversation-header">
              <span aria-hidden="true">‹</span>
              <strong>phoenix-coding-agent-design</strong>
              <span className="work-actions-conversation-header-state">● ready</span>
            </header>
            <section className="work-actions-conversation-transcript" aria-label="Conversation transcript preview">
              <article className="work-actions-conversation-message">
                <p>The implementation is complete and ready for review.</p>
                <p>The important nuance is that the workspace changes still belong to this conversation until its final work action is complete.</p>
                <h2>{scenario.title}</h2>
                <p>{scenario.description}</p>
              </article>
            </section>
            <WorkControlBar
              conversationId={`fixture-${scenario.id}`}
              convModeLabel={scenario.convModeLabel}
              phaseType={scenario.phaseType}
              continuedInConvId={scenario.continuedInConvId}
              {...(scenario.canSendMessage ? { onSendMessage: async () => {} } : {})}
              prStatusHandle={handle}
            />
            <section className="work-actions-conversation-input" aria-label="Message input preview">
              <span>Type a message…</span>
              <span aria-hidden="true">◉</span>
              <button type="button" aria-label="Send message">➤</button>
            </section>
            <footer className="work-actions-conversation-statebar">
              <span>‹</span>
              <strong>phoenix-coding-agent-design</strong>
              <span className="work-actions-conversation-ready">● ready</span>
              <span>▱</span>
            </footer>
          </main>
        </ViewerSlotProvider>
      </MemoryRouter>
    </ThemeContext.Provider>
  );
}
