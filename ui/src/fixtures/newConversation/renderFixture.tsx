import { useEffect, useRef, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationProvider } from '../../conversation';
import { NewConversationPage } from '../../pages/NewConversationPage';
import '../../index.css';
import { installNewConversationFixtureApi } from './mockApi';
import type { NewConversationScenario } from './types';

interface Props {
  scenario: NewConversationScenario;
}

const STORAGE_KEYS = [
  'phoenix-last-cwd',
  'phoenix-last-model',
  'phoenix-new-conversation-draft',
  'phoenix-recent-dirs',
] as const;

function pageHasSettled(root: HTMLElement, scenario: NewConversationScenario): boolean {
  const directory = root.querySelector<HTMLInputElement>('.directory-picker input');
  const model = root.querySelector<HTMLSelectElement>('.settings-select');
  const draft = root.querySelector<HTMLTextAreaElement>('.new-conv-textarea-mobile');
  const directoryReady = root.querySelector('.status-ok') !== null;
  const workflowReady = Array.from(root.querySelectorAll('.workflow-card-content strong'))
    .some((element) => element.textContent === 'Chat in a fresh worktree');

  return directory?.value === scenario.cwd
    && model?.value === scenario.models.default
    && draft?.value === scenario.draft
    && directoryReady
    && workflowReady;
}

function NewConversationFixtureBody({ scenario }: Props) {
  const rootRef = useRef<HTMLElement>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    document.documentElement.dataset['theme'] = scenario.theme;
  }, [scenario.theme]);
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return undefined;

    const update = () => setReady(pageHasSettled(root, scenario));
    update();
    const observer = new MutationObserver(update);
    observer.observe(root, { attributes: true, childList: true, subtree: true });
    return () => observer.disconnect();
  }, [scenario]);

  return (
    <main
      ref={rootRef}
      data-new-conversation-fixture={scenario.id}
      {...(ready ? { 'data-new-conversation-fixture-ready': scenario.id } : {})}
    >
      <NewConversationPage />
    </main>
  );
}

export function NewConversationFixture({ scenario }: Props) {
  const [installed, setInstalled] = useState(false);

  useEffect(() => {
    const previous = new Map(STORAGE_KEYS.map((key) => [key, localStorage.getItem(key)]));
    const restoreApi = installNewConversationFixtureApi(scenario);
    localStorage.setItem('phoenix-last-cwd', scenario.cwd);
    localStorage.setItem('phoenix-last-model', scenario.models.default);
    localStorage.setItem('phoenix-new-conversation-draft', scenario.draft);
    localStorage.setItem('phoenix-recent-dirs', JSON.stringify(scenario.recentDirs));
    setInstalled(true);

    return () => {
      restoreApi();
      for (const [key, value] of previous) {
        if (value === null) localStorage.removeItem(key);
        else localStorage.setItem(key, value);
      }
    };
  }, [scenario]);

  if (!installed) return null;
  return (
    <MemoryRouter initialEntries={['/new']}>
      <ConversationProvider>
        <NewConversationFixtureBody scenario={scenario} />
      </ConversationProvider>
    </MemoryRouter>
  );
}
