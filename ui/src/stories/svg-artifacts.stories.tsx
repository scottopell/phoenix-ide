import { useMemo } from 'react';
import { SharePage } from '../pages/SharePage';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ToolOnlyAgentTurnGroup } from '../components/MessageComponents';
import type { Message } from '../api';
import { DensityContext, type Density } from '../hooks/useDensity';
import '../index.css';

const description = 'Measured directory sizes in GiB; free space is shown separately.';
function Fixture({ theme, density, shape }: { theme: 'light' | 'dark'; density: Density; shape?: 'tall' | 'wide' }) {
  document.documentElement.dataset['theme'] = theme;
  const artifact = { artifact_id: shape ?? 'chart', conversation_id: 'svg-fixture', title: 'Largest storage consumers — measured sizes with exact GiB labels', description, width: shape === 'tall' ? 400 : shape === 'wide' ? 8000 : 800, height: shape === 'tall' ? 1600 : 400, validation: 'accepted_static_svg' };
  const agent: Message = { message_id: 'agent', conversation_id: 'svg-fixture', sequence_id: 1, message_type: 'agent', content: [{ type: 'tool_use', id: 'publish', name: 'present_svg', input: { path: '/staging/chart.svg' } }], display_data: null, created_at: '' };
  const result: Message = { ...agent, message_id: 'result', message_type: 'tool', sequence_id: 2, content: { tool_use_id: 'publish', content: JSON.stringify(artifact), is_error: false } };
  return <main style={{ maxWidth: 1000, padding: 12, margin: 'auto' }} data-svg-artifacts-ready={shape ?? `${theme}-${density}`}>
    <DensityContext.Provider value={{ density, setDensity: () => {} }}><MemoryRouter>
      <ToolOnlyAgentTurnGroup members={[{ kind: 'agent_turn', key: 'agent', agent, toolResultsByUseId: new Map([['publish', result]]), isFirstInTurn: true }]} />
    </MemoryRouter></DensityContext.Provider>
  </main>;
}
export const LightFull = () => <Fixture theme="light" density="full" />;
LightFull.storyName = 'light-full';
export const DarkFull = () => <Fixture theme="dark" density="full" />;
DarkFull.storyName = 'dark-full';
export const LightCompact = () => <Fixture theme="light" density="compact" />;
LightCompact.storyName = 'light-compact';
export const DarkCompact = () => <Fixture theme="dark" density="compact" />;
DarkCompact.storyName = 'dark-compact';

export const Tall = () => <Fixture theme="light" density="full" shape="tall" />;
Tall.storyName = 'tall';
export const Wide = () => <Fixture theme="dark" density="compact" shape="wide" />;
Wide.storyName = 'wide';

export function Shared() {
  const store = useMemo(() => new ConversationStore(), []);
  return <div data-svg-artifacts-ready="shared">
    <ConversationContext.Provider value={store}><MemoryRouter initialEntries={['/s/anonymous-svg']}>
      <Routes><Route path="/s/:token" element={<SharePage />} /></Routes>
    </MemoryRouter></ConversationContext.Provider>
  </div>;
}
Shared.storyName = 'shared';
