import { useEffect, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationList } from '../../components/ConversationList';
import '../../index.css';
import { conversationPanelFixtureData } from './scenarios';
import type { ConversationPanelScenario } from './types';

interface Props {
  scenario: ConversationPanelScenario;
  showToolbar?: boolean;
}

function stateDotClass(presentationMode: string | undefined): string {
  switch (presentationMode) {
    case 'working': return 'working';
    case 'error': return 'error';
    case 'done': return 'terminal';
    case 'needs_action': return 'awaiting-approval';
    default: return 'idle';
  }
}

export function ConversationPanelFixture({ scenario, showToolbar = true }: Props) {
  const [showArchived, setShowArchived] = useState(scenario.kind === 'archived');

  useEffect(() => {
    delete document.documentElement.dataset['conversationPanelFixtureReady'];
    document.documentElement.dataset['theme'] = scenario.theme;
    if (scenario.kind === 'archived') setShowArchived(true);
    const timer = window.setTimeout(() => {
      document.documentElement.dataset['conversationPanelFixtureReady'] = scenario.id;
    }, 50);
    return () => {
      window.clearTimeout(timer);
      delete document.documentElement.dataset['conversationPanelFixtureReady'];
    };
  }, [scenario]);

  return (
    <MemoryRouter initialEntries={[`/c/${conversationPanelFixtureData.activeSlug}`]}>
      <main className="fixture-page" data-conversation-panel-fixture={scenario.id}>
        {showToolbar && (
          <div className="fixture-toolbar">
            <strong>Conversation panel fixture</strong>
            <span>scenario={scenario.id}</span>
            <span>theme={scenario.theme}</span>
          </div>
        )}
        <div className="fixture-panel-stage conversation-panel-fixture-stage">
          {scenario.collapsed ? (
            <aside className="sidebar sidebar-collapsed" style={{ width: `${scenario.width}px`, minWidth: `${scenario.width}px` }}>
              <button className="sidebar-toggle" title="Expand sidebar">›</button>
              <div className="sidebar-collapsed-dots">
                {conversationPanelFixtureData.conversations.map((conv) => (
                  <button key={conv.id} className={`sidebar-dot-btn ${conv.slug === conversationPanelFixtureData.activeSlug ? 'active' : ''}`} title={conv.slug}>
                    <span className={`conv-state-dot ${stateDotClass(conv.presentation_mode)}`} />
                  </button>
                ))}
              </div>
            </aside>
          ) : (
            <aside
              className="sidebar sidebar-expanded conversation-panel-fixture-sidebar"
              style={{ width: `${scenario.width}px`, minWidth: `${scenario.width}px` }}
            >
              <div className="sidebar-header">
                <button className="sidebar-toggle-expanded" title="Collapse sidebar">‹</button>
                <button className="sidebar-brand">
                  <img src="/phoenix.svg" alt="Phoenix" className="sidebar-logo" />
                  <span className="sidebar-brand-text">Phoenix</span>
                </button>
                <button className="btn-primary sidebar-new-btn">+ New</button>
              </div>
              <div className="project-tabs">
                <button className="project-tab active">All</button>
                <button className="project-tab">phoenix-ide</button>
              </div>
              <div className="sidebar-list">
                <ConversationList
                  conversations={conversationPanelFixtureData.conversations}
                  archivedConversations={conversationPanelFixtureData.archivedConversations}
                  showArchived={showArchived}
                  onToggleArchived={() => setShowArchived((value) => !value)}
                  onNewConversation={() => {}}
                  onArchive={() => {}}
                  onDelete={() => {}}
                  onRename={() => {}}
                  onArchiveChain={() => {}}
                  onDeleteChain={() => {}}
                  onConversationClick={() => {}}
                  activeSlug={conversationPanelFixtureData.activeSlug}
                  sidebarMode
                />
              </div>
            </aside>
          )}
        </div>
      </main>
    </MemoryRouter>
  );
}
