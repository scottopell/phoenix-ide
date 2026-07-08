import { useEffect, useMemo, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { Sidebar } from '../../components/Sidebar';
import '../../index.css';
import { installSidebarFixtureApi } from './mockApi';
import { sidebarFixtureData } from './scenarios';
import type { SidebarScenario } from './types';

interface Props {
  scenario: SidebarScenario;
}

function SidebarFixtureBody({ scenario }: Props) {
  if (scenario.initialProjectId) {
    localStorage.setItem('phoenix:sidebar-project-filter', scenario.initialProjectId);
  } else {
    localStorage.removeItem('phoenix:sidebar-project-filter');
  }

  const restoreApi = useMemo(() => installSidebarFixtureApi(sidebarFixtureData), []);
  const [collapsed, setCollapsed] = useState(scenario.collapsed);

  useEffect(() => restoreApi, [restoreApi]);

  useEffect(() => {
    document.documentElement.dataset['theme'] = scenario.theme;
    setCollapsed(scenario.collapsed);
  }, [scenario]);

  return (
    <main className="sidebar-fixture" data-sidebar-fixture={scenario.id}>
      <Sidebar
        key={scenario.id}
        collapsed={collapsed}
        onToggle={() => setCollapsed((value) => !value)}
        conversations={sidebarFixtureData.conversations}
        archivedConversations={sidebarFixtureData.archivedConversations}
        activeSlug={scenario.activeSlug}
        onConversationCreated={() => {}}
        width={320}
      />
      <section className="sidebar-fixture-notes" aria-label="Fixture notes">
        <h1 className="sidebar-fixture-title">{scenario.id}</h1>
        <p className="sidebar-fixture-copy">Project scope, lifecycle counts, empty states, and collapsed overflow are fixture-backed.</p>
      </section>
    </main>
  );
}

export function SidebarFixture({ scenario }: Props) {
  return (
    <MemoryRouter initialEntries={scenario.activeSlug ? [`/c/${scenario.activeSlug}`] : ['/']}>
      <SidebarFixtureBody scenario={scenario} />
    </MemoryRouter>
  );
}
