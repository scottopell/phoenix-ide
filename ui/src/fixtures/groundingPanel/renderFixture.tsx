import { useEffect, useMemo, useState } from 'react';
import { FileExplorerPanel, FileExplorerProvider } from '../../components/FileExplorer';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import '../../index.css';
import { fixtureWorkScope, installGroundingPanelFixtureFetch } from './mockApi';
import type { GroundingPanelScenario } from './types';

interface Props {
  scenario: GroundingPanelScenario;
  showToolbar?: boolean;
}

export function GroundingPanelFixture({ scenario, showToolbar = true }: Props) {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    setReady(false);
    delete document.documentElement.dataset['groundingFixtureReady'];
    document.documentElement.dataset['theme'] = scenario.theme;
    const restore = installGroundingPanelFixtureFetch(scenario);
    setReady(true);
    return restore;
  }, [scenario]);

  const liveWorkScope = useMemo(() => fixtureWorkScope(scenario), [scenario]);

  useEffect(() => {
    if (!ready) return;
    const timer = window.setTimeout(() => {
      const headers = [...document.querySelectorAll<HTMLButtonElement>('.grounding-section-header')];
      if (scenario.kind === 'full' || scenario.kind === 'empty' || scenario.kind === 'errors') {
        for (const label of ['MCP', 'Skills', 'Tasks']) {
          headers.find((el) => el.textContent?.includes(label))?.click();
        }
        return;
      }
      if (scenario.kind === 'skill-detail') {
        headers.find((el) => el.textContent?.includes('Skills'))?.click();
        window.setTimeout(() => document.querySelector<HTMLElement>('.skill-item')?.click(), 100);
      } else if (scenario.kind === 'task-detail') {
        headers.find((el) => el.textContent?.includes('Tasks'))?.click();
        window.setTimeout(() => document.querySelector<HTMLElement>('.tasks-item')?.click(), 100);
      }
    }, 250);
    return () => window.clearTimeout(timer);
  }, [scenario, ready]);

  useEffect(() => {
    if (!ready) return;
    const timer = window.setTimeout(() => {
      document.documentElement.dataset['groundingFixtureReady'] = scenario.id;
    }, 700);
    return () => {
      window.clearTimeout(timer);
      delete document.documentElement.dataset['groundingFixtureReady'];
    };
  }, [scenario.id, ready]);

  if (!ready) return null;

  return (
    <ViewerSlotProvider scopeKey="grounding-fixture" browserSessionActive={false}>
      <FileExplorerProvider>
        <main className="fixture-page" data-grounding-fixture={scenario.id}>
          {showToolbar && (
            <div className="fixture-toolbar">
              <strong>Grounding panel fixture</strong>
              <span>scenario={scenario.id}</span>
              <span>theme={scenario.theme}</span>
            </div>
          )}
          <div className="fixture-panel-stage">
            <FileExplorerPanel
              collapsed={scenario.collapsed}
              onToggle={() => {}}
              rootPath={scenario.rootPath}
              conversationId={scenario.conversationId}
              showToast={() => {}}
              showError={() => {}}
              branchName={scenario.branchName}
              activeSlug={scenario.activeSlug}
              width={scenario.width}
              workScopeKey={scenario.scopeKey}
              liveWorkScope={liveWorkScope}
            />
          </div>
        </main>
      </FileExplorerProvider>
    </ViewerSlotProvider>
  );
}
