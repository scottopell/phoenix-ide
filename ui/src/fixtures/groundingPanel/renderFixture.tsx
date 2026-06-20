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

  // Signal capture-readiness off the scenario's settled DOM, not a wall-clock
  // timer: the detail scenarios open their viewer via an async click→fetch→render
  // chain, so a fixed delay can flip "ready" while the list is still showing and
  // produce a silently-wrong screenshot. Poll for the settled marker; if it
  // never arrives, mark ready anyway but warn (a non-fatal console.warn, so the
  // miss is visible without failing the run).
  useEffect(() => {
    if (!ready) return;
    let cancelled = false;

    const isSettled = (): boolean => {
      switch (scenario.kind) {
        case 'skill-detail':
          return document.querySelector('.skill-viewer') != null;
        case 'task-detail':
          return document.querySelector('.task-viewer') != null;
        case 'full':
        case 'empty':
        case 'errors':
          return document.querySelector('.grounding-section-body') != null;
        default:
          return document.querySelector('.grounding-section') != null;
      }
    };

    const deadline = Date.now() + 6000;
    const markReady = () => {
      if (!cancelled) document.documentElement.dataset['groundingFixtureReady'] = scenario.id;
    };
    let timer = 0;
    const poll = () => {
      if (cancelled) return;
      if (isSettled()) return markReady();
      if (Date.now() >= deadline) {
        console.warn(`grounding fixture "${scenario.id}" did not reach its settled state before deadline; capturing as-is`);
        return markReady();
      }
      timer = window.setTimeout(poll, 50);
    };
    timer = window.setTimeout(poll, 50);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      delete document.documentElement.dataset['groundingFixtureReady'];
    };
  }, [scenario, ready]);

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
