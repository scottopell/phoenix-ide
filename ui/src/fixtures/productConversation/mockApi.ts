import { api, streamApi, type ChainSseEventData } from '../../api';
import type { ProductConversationScenario } from './types';

export function installProductConversationFixtureApi(scenario: ProductConversationScenario): () => void {
  const originalGetProductConversationSnapshot = api.getProductConversationSnapshot;
  const originalGetConversationRouteBySlug = api.getConversationRouteBySlug;
  const originalGetConversationRoute = api.getConversationRoute;
  const originalGetConversation = api.getConversation;
  const originalGetConversationBySlug = api.getConversationBySlug;
  const originalGetChain = api.getChain;
  const originalSubmitChainQuestion = api.submitChainQuestion;
  const originalSubscribeToChainStream = streamApi.subscribeToChainStream;

  api.getProductConversationSnapshot = async () => {
    if (scenario.state === 'loading') {
      return new Promise(() => {}) as ReturnType<typeof api.getProductConversationSnapshot>;
    }
    if (scenario.state === 'error') {
      throw new Error(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot');
    }
    if (!scenario.snapshot) {
      throw new Error(`Scenario ${scenario.id} is missing snapshot data`);
    }
    return scenario.snapshot;
  };

  api.getConversationRouteBySlug = async (slug: string) => ({ id: slug, slug });
  api.getConversationRoute = async (id: string) => ({ id, slug: id });
  const fixtureConversation = (id: string) => {
    const segment = scenario.snapshot?.segments.find((candidate) => candidate.transcript_row_id === id)
      ?? scenario.snapshot?.segments.at(-1);
    return {
      conversation: {
        id,
        slug: id,
        title: segment?.title ?? scenario.title,
        model: 'gpt-5',
        cwd: scenario.snapshot?.work_identity?.worktree_path ?? '/tmp',
        created_at: scenario.snapshot?.updated_at ?? new Date(0).toISOString(),
        updated_at: scenario.snapshot?.updated_at ?? new Date(0).toISOString(),
        message_count: segment?.messages.length ?? 0,
        state: { type: 'idle' as const },
      },
      messages: segment?.messages ?? [],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    };
  };
  api.getConversation = async (id: string) => fixtureConversation(id);
  api.getConversationBySlug = async (slug: string) => fixtureConversation(slug);

  api.getChain = async () => {
    if (!scenario.chain) {
      throw new Error(`Scenario ${scenario.id} does not provide chain data`);
    }
    return scenario.chain;
  };

  api.submitChainQuestion = async () => ({ chain_qa_id: 'fixture-chain-qa' });

  streamApi.subscribeToChainStream = (
    rootId: string,
    onEvent: (event: ChainSseEventData) => void,
    onError?: (err: unknown) => void,
  ) => {
    void rootId;
    void onEvent;
    void onError;
    return { close() {} } as EventSource;
  };

  return () => {
    api.getProductConversationSnapshot = originalGetProductConversationSnapshot;
    api.getConversationRouteBySlug = originalGetConversationRouteBySlug;
    api.getConversationRoute = originalGetConversationRoute;
    api.getConversation = originalGetConversation;
    api.getConversationBySlug = originalGetConversationBySlug;
    api.getChain = originalGetChain;
    api.submitChainQuestion = originalSubmitChainQuestion;
    streamApi.subscribeToChainStream = originalSubscribeToChainStream;
  };
}
