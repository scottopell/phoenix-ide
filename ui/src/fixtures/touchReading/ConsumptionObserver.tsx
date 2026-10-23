import { useContext, useEffect } from 'react';
import { ConversationContext } from '../../conversation/ConversationContext';
import { observeConsumption, type ConsumptionReceipt } from './observeConsumption';

export type ConsumptionController = ReturnType<typeof observeConsumption>;

export function ConsumptionObserver({ liveId, publish, attach }: {
  liveId: string;
  publish: (receipt: ConsumptionReceipt) => void;
  attach: (controller: ConsumptionController | null) => void;
}) {
  const store = useContext(ConversationContext);
  useEffect(() => {
    if (!store) {
      publish({ status: 'UNOBSERVED', phase: null, reason: 'provider unavailable' });
      attach(null);
      return;
    }
    const controller = observeConsumption(store, liveId, publish);
    attach(controller);
    return () => {
      attach(null);
      controller.dispose();
    };
  }, [store, liveId, publish, attach]);
  return null;
}
