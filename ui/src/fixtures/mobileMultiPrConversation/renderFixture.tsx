import { useEffect, useMemo } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { StateBar } from '../../components/StateBar';
import type { ConversationPrStatusHandle } from '../../hooks/useConversationPrStatus';
import '../../index.css';
import './renderFixture.css';
import {
  mobileMultiPrConversation,
  mobileMultiPrSelection,
  mobileMultiPrStatus,
} from './scenarios';
import type { MobileMultiPrConversationScenario } from './types';

interface Props {
  scenario: MobileMultiPrConversationScenario;
}

function markReadyWhenRendered(scenario: MobileMultiPrConversationScenario): () => void {
  let cancelled = false;
  let observer: MutationObserver | null = null;

  const settle = () => {
    if (cancelled) return;
    const stateBar = document.querySelector<HTMLElement>('#state-bar.statebar-mobile');
    if (!stateBar) return;

    if (scenario.expanded && stateBar.getAttribute('aria-expanded') !== 'true') {
      stateBar.querySelector<HTMLButtonElement>('.statebar-chevron')?.click();
      return;
    }

    const chooser = stateBar.querySelector<HTMLButtonElement>('[data-testid="active-pr-selector-trigger"]');
    if (scenario.chooserOpen && chooser?.getAttribute('aria-expanded') !== 'true') {
      chooser?.click();
      return;
    }

    if (scenario.chooserOpen && !stateBar.querySelector('[aria-label="Active pull request choices"]')) return;

    document.documentElement.dataset['mobileMultiPrConversationFixtureReady'] = scenario.id;
    observer?.disconnect();
  };

  observer = new MutationObserver(settle);
  observer.observe(document.body, { attributes: true, childList: true, subtree: true });
  settle();

  return () => {
    cancelled = true;
    observer?.disconnect();
  };
}

export function MobileMultiPrConversationFixture({ scenario }: Props) {
  const prStatusHandle = useMemo<ConversationPrStatusHandle>(() => ({
    state: { status: 'ready', prStatus: mobileMultiPrStatus },
    refresh: async () => mobileMultiPrStatus,
    activeSelection: mobileMultiPrSelection,
    activePrSummary: null,
    ambiguous: true,
    pinActivePr: async () => undefined,
    resumeInference: async () => undefined,
  }), []);

  useEffect(() => {
    const previousTheme = document.documentElement.dataset['theme'];
    delete document.documentElement.dataset['mobileMultiPrConversationFixtureReady'];
    document.documentElement.dataset['theme'] = 'dark';
    const stopSettling = markReadyWhenRendered(scenario);
    return () => {
      stopSettling();
      delete document.documentElement.dataset['mobileMultiPrConversationFixtureReady'];
      if (previousTheme === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, [scenario]);

  return (
    <MemoryRouter initialEntries={[`/c/${mobileMultiPrConversation.slug}`]}>
      <main
        id="app"
        className="mobile-multi-pr-conversation-fixture"
        data-mobile-multi-pr-conversation-fixture={scenario.id}
      >
        <section className="mobile-multi-pr-conversation-fixture-transcript" aria-label="Conversation transcript">
          <div className="mobile-multi-pr-conversation-fixture-message">
            <strong>Assistant</strong>
            <p>The multi-PR association is implemented. I’m ready to review the mobile experience.</p>
          </div>
        </section>
        <section className="mobile-multi-pr-conversation-fixture-input" aria-label="Message input preview">
          <span>Message Phoenix…</span>
          <button type="button">Send</button>
        </section>
        <StateBar
          conversation={mobileMultiPrConversation}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={48_000}
          modelContextWindow={200_000}
          onOpenFiles={() => undefined}
          prStatusHandle={prStatusHandle}
        />
      </main>
    </MemoryRouter>
  );
}
