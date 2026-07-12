export interface ActivePrSelectorIntent {
  owner: symbol;
  requestOpen: () => void;
}

let activePrSelectorIntent: ActivePrSelectorIntent | null = null;

export function setActivePrSelectorIntent(intent: ActivePrSelectorIntent | null): () => void {
  activePrSelectorIntent = intent;
  return () => {
    if (intent !== null && activePrSelectorIntent?.owner === intent.owner) {
      activePrSelectorIntent = null;
    }
  };
}

export function requestActivePrSelectorOpen() {
  activePrSelectorIntent?.requestOpen();
}
