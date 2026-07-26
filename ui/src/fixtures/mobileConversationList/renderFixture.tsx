import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationList } from '../../components/ConversationList';
import { StorageStatus } from '../../components/StorageStatus';
import { SettingsDropdown } from '../../components/SettingsDropdown';
import { useAppTouchContainment, useDocumentViewportOwnership } from '../../components/viewportRoutes';
import '../../index.css';
import { getMobileConversationListFixtureData } from './scenarios';
import type { MobileConversationListScenario } from './types';

interface Props {
  scenario: MobileConversationListScenario;
}

export function MobileConversationListFixtureBody({ scenario }: Props) {
  useDocumentViewportOwnership(true);
  useAppTouchContainment(true);
  const [showArchived, setShowArchived] = useState(scenario.kind === 'archived');
  const fixtureData = getMobileConversationListFixtureData(scenario);
  const totalConversations = fixtureData.conversations.length + fixtureData.archivedConversations.length;

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
      <main id="main-area" data-app-scroll-owner>
        <ConversationList
          conversations={fixtureData.conversations}
          archivedConversations={fixtureData.archivedConversations}
          showArchived={showArchived}
          onToggleArchived={() => setShowArchived((value) => !value)}
          onNewConversation={() => {}}
          onArchive={() => {}}
          onDelete={() => {}}
          onRename={() => {}}
          onArchiveChain={() => {}}
          onDeleteChain={() => {}}
          onConversationClick={() => {}}
          activeSlug={scenario.activeSlug}
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
          footer={<StorageStatus conversationCount={totalConversations} />}
        />
      </main>
    </div>
  );
}

export function MobileConversationListFixture({ scenario }: Props) {
  return (
    <MemoryRouter initialEntries={scenario.activeSlug ? [`/c/${scenario.activeSlug}`] : ['/']}>
      <MobileConversationListFixtureBody scenario={scenario} />
    </MemoryRouter>
  );
}
