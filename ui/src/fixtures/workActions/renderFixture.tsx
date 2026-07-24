import { MemoryRouter } from 'react-router-dom';
import { ThemeContext } from '../../hooks/useTheme';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { WorkControlBar } from '../../components/WorkActions';
import type { WorkActionsScenario } from './types';
import '../../index.css';

interface Props {
  scenario: WorkActionsScenario;
}

const noop = () => {};

export function WorkActionsFixture({ scenario }: Props) {
  const handle = {
    state: scenario.prState,
    refresh: async () => undefined,
  };

  return (
    <ThemeContext.Provider value={{ theme: 'dark', toggleTheme: noop }}>
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <main className="fixture-page" data-work-actions-fixture={scenario.id}>
            <div className="fixture-toolbar">
              <strong>Work Actions fixture</strong>
              <span>{scenario.title}</span>
            </div>
            <p className="fixture-description">{scenario.description}</p>
            <section className="work-actions-fixture-stage">
              <WorkControlBar
                conversationId={`fixture-${scenario.id}`}
                convModeLabel={scenario.convModeLabel}
                phaseType={scenario.phaseType}
                continuedInConvId={scenario.continuedInConvId}
                prStatusHandle={handle}
              />
            </section>
          </main>
        </ViewerSlotProvider>
      </MemoryRouter>
    </ThemeContext.Provider>
  );
}
