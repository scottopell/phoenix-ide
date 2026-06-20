import { getGroundingPanelScenario, GroundingPanelFixture } from '../fixtures/groundingPanel';
import type { GroundingPanelTheme } from '../fixtures/groundingPanel/types';

function parseTheme(value: string | null): GroundingPanelTheme | null {
  return value === 'light' || value === 'dark' ? value : null;
}

export function GroundingPanelFixturePage() {
  const params = new URLSearchParams(window.location.search);
  const scenario = getGroundingPanelScenario(params.get('scenario') ?? params.get('id'));
  const theme = parseTheme(params.get('theme'));
  const resolvedScenario = theme ? { ...scenario, theme } : scenario;
  return <GroundingPanelFixture scenario={resolvedScenario} />;
}
