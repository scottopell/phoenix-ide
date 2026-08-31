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
] as const;

function pageHasSettled(root: HTMLElement, scenario: NewConversationScenario): boolean {
  const directory = root.querySelector<HTMLInputElement>('.directory-picker input');
  const model = root.querySelector<HTMLSelectElement>('.settings-select');
  const draft = root.querySelector<HTMLTextAreaElement>('.new-conv-textarea-mobile');
  const directoryReady = root.querySelector('.status-ok') !== null;
  const sendButtons = Array.from(root.querySelectorAll<HTMLButtonElement>('.new-conv-send'));
  const recoveryItems = root.querySelectorAll('.product-creation-recovery__item').length;
  const expectedRecoveryItems = (scenario.recoveryRows?.length ?? 0) * 2;
  const loadMore = Array.from(root.querySelectorAll<HTMLButtonElement>('.product-creation-recovery__action'))
    .find((button) => button.textContent === 'Load more');
  const recoveryReady = recoveryItems >= expectedRecoveryItems
    && (expectedRecoveryItems === 0 || root.querySelector('[aria-label="Recent product creation attempts"]') !== null)
    && (scenario.recoveryNextCursor ? loadMore !== undefined : loadMore === undefined);
  if (expectedRecoveryItems > 0) return recoveryReady;

  return directory?.value === scenario.cwd
    && model?.value === scenario.models.default
    && draft?.value === scenario.draft
    && directoryReady
    && sendButtons.length === 2
    && sendButtons.every((button) => !button.disabled)
    && !root.textContent?.includes('Workflow')
    && !root.textContent?.includes('Chat in a fresh worktree')
    && recoveryReady;
}

function NewConversationFixtureBody({ scenario }: Props) {
  const rootRef = useRef<HTMLElement>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const previousTheme = document.documentElement.getAttribute('data-theme');
    document.documentElement.dataset['theme'] = scenario.theme;
    return () => {
      if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, [scenario.theme]);
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return undefined;

    let interval = 0;
    const checkUntilSettled = () => {
      if (!pageHasSettled(root, scenario)) return;
      setReady(true);
      window.clearInterval(interval);
    };
    checkUntilSettled();
    interval = window.setInterval(checkUntilSettled, 16);
    return () => window.clearInterval(interval);
  }, [scenario]);

  return (
    <main
      ref={rootRef}
      data-new-conversation-fixture={scenario.id}
      data-new-conversation-scenario={scenario.id}
      data-new-conversation-recovery-count={String(scenario.recoveryRows?.length ?? 0)}
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
