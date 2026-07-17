import { useEffect, useMemo } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { StateBar } from '../../components/StateBar';
import { WorkControlBar } from '../../components/WorkActions';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import type { ConversationPrStatusHandle } from '../../hooks/useConversationPrStatus';
import {
  mobileMultiPrActiveSelection,
  mobileMultiPrActiveStatus,
  mobileMultiPrAssociatedPrs,
  mobileMultiPrConversation,
  mobileMultiPrSelection,
  mobileMultiPrStatus,
} from '../mobileMultiPrConversation/scenarios';
import '../../index.css';
import './renderFixture.css';

export type DesktopMultiPrScenario = 'collapsed-two-open' | 'expanded-active-feedback' | 'ambiguous-two-open';

function settleFixture(scenario: DesktopMultiPrScenario): () => void {
  let observer: MutationObserver | null = null;
  const settle = () => {
    if (scenario === 'expanded-active-feedback') {
      const activeChip = document.querySelector<HTMLButtonElement>('.desktop-pr-dock .mobile-pr-chip--active');
      if (activeChip?.getAttribute('aria-expanded') !== 'true') {
        activeChip?.click();
        return;
      }
    }
    if (!document.querySelector('[data-testid="desktop-work-controls"]')) return;
    document.documentElement.dataset['desktopMultiPrConversationFixtureReady'] = scenario;
    const fixture = document.querySelector<HTMLElement>('[data-desktop-multi-pr-conversation-fixture]');
    if (fixture) fixture.dataset['desktopMultiPrConversationFixtureReady'] = scenario;
    observer?.disconnect();
  };
  observer = new MutationObserver(settle);
  observer.observe(document.body, { attributes: true, childList: true, subtree: true });
  settle();
  return () => observer?.disconnect();
}

export function DesktopMultiPrConversationFixture({ scenario }: { scenario: DesktopMultiPrScenario }) {
  const ambiguous = scenario === 'ambiguous-two-open';
  const handle = useMemo<ConversationPrStatusHandle>(() => ({
    state: { status: 'ready', prStatus: ambiguous ? mobileMultiPrStatus : mobileMultiPrActiveStatus },
    refresh: async () => ambiguous ? mobileMultiPrStatus : mobileMultiPrActiveStatus,
    activeSelection: ambiguous ? mobileMultiPrSelection : mobileMultiPrActiveSelection,
    activePrSummary: ambiguous ? null : mobileMultiPrAssociatedPrs[1]!,
    ambiguous,
    pinActivePr: async () => undefined,
    resumeInference: async () => undefined,
  }), [ambiguous]);

  useEffect(() => {
    const previousTheme = document.documentElement.dataset['theme'];
    delete document.documentElement.dataset['desktopMultiPrConversationFixtureReady'];
    document.documentElement.dataset['theme'] = 'dark';
    const stopSettling = settleFixture(scenario);
    return () => {
      stopSettling();
      delete document.documentElement.dataset['desktopMultiPrConversationFixtureReady'];
      if (previousTheme === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, [scenario]);

  return (
    <MemoryRouter initialEntries={[`/c/${mobileMultiPrConversation.slug}`]}>
      <main id="app" className="desktop-multi-pr-conversation-fixture" data-desktop-multi-pr-conversation-fixture={scenario}>
        <StateBar
          conversation={mobileMultiPrConversation}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={48_000}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
        <section className="desktop-multi-pr-conversation-transcript" aria-label="Conversation transcript">
          <div>
            <strong>Assistant</strong>
            <p>The multi-PR association is implemented. Select a pull request below to inspect its next actions.</p>
          </div>
        </section>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar
            conversationId={mobileMultiPrConversation.id}
            convModeLabel="Work"
            phaseType="idle"
            continuedInConvId={null}
            onSendMessage={async () => undefined}
            prStatusHandle={handle}
          />
        </ViewerSlotProvider>
        <section className="desktop-multi-pr-conversation-input" aria-label="Message input preview">
          <span>Message Phoenix…</span><button type="button">Send</button>
        </section>
      </main>
    </MemoryRouter>
  );
}
