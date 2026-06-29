import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationList } from '../../components/ConversationList';
import { SettingsDropdown } from '../../components/SettingsDropdown';
import '../../index.css';
import { mobileConversationListFixtureData } from './scenarios';
import type { MobileConversationListScenario } from './types';

interface Props {
  scenario: MobileConversationListScenario;
}

export function MobileConversationListFixtureBody({ scenario }: Props) {
  const [showArchived, setShowArchived] = useState(scenario.kind === 'archived');

  useEffect(() => {
    delete document.documentElement.dataset['mobileConversationListFixtureReady'];
    document.documentElement.dataset['theme'] = scenario.theme;
    setShowArchived(scenario.kind === 'archived');
    const timer = window.setTimeout(() => {
      document.documentElement.dataset['mobileConversationListFixtureReady'] = scenario.id;
    }, 50);
    return () => {
      window.clearTimeout(timer);
      delete document.documentElement.dataset['mobileConversationListFixtureReady'];
    };
  }, [scenario]);

  return (
    <div id="app" className="list-page mobile-conversation-list-fixture">
      <main id="main-area">
        <ConversationList
          conversations={mobileConversationListFixtureData.conversations}
          archivedConversations={mobileConversationListFixtureData.archivedConversations}
          showArchived={showArchived}
          onToggleArchived={() => setShowArchived((value) => !value)}
          onNewConversation={() => {}}
          onArchive={() => {}}
          onDelete={() => {}}
          onRename={() => {}}
          onArchiveChain={() => {}}
          onDeleteChain={() => {}}
          onConversationClick={() => {}}
          listDensity="mobile"
          authChip={<span className="mobile-conversation-list-fixture-auth">✓</span>}
          utilityActions={(
            <SettingsDropdown
              theme={scenario.theme}
              onToggleTheme={() => {}}
              codexPreflight={null}
              onPreflightInvalidated={() => {}}
              compact
            />
          )}
        />
      </main>
    </div>
  );
}

export function MobileConversationListFixture({ scenario }: Props) {
  return (
    <MemoryRouter>
      <MobileConversationListFixtureBody scenario={scenario} />
    </MemoryRouter>
  );
}
