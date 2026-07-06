import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { MessageList } from '../../components/MessageList';
import '../../index.css';
import type { MessageListScenario } from './types';
import { messageListFixtureData } from './scenarios';

interface Props {
  scenario: MessageListScenario;
}

export function MessageListFixture({ scenario }: Props) {
  const [ready, setReady] = useState(false);
  const data = messageListFixtureData(scenario);
  const store = new ConversationStore();

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
            </div>
            <div className="fixture-message-list-stage">
              <div className="message-list-fixture-shell">
                <MessageList
                  messages={data.messages}
                  pendingMessages={data.pendingMessages}
                  convState={data.convState}
                  onRetry={() => {}}
                  onOpenFile={() => {}}
                  conversationId={data.conversationId}
                  slug={data.slug}
                />
              </div>
            </div>
          </main>
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
