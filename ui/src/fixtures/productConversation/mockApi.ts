import { api, streamApi, type ChainSseEventData } from '../../api';
import type { ProductConversationScenario } from './types';

export function installProductConversationFixtureApi(scenario: ProductConversationScenario): () => void {
  const originalGetProductConversationSnapshot = api.getProductConversationSnapshot;
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
    api.getChain = originalGetChain;
    api.submitChainQuestion = originalSubmitChainQuestion;
    streamApi.subscribeToChainStream = originalSubscribeToChainStream;
  };
}
