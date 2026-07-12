export interface ActivePrSelectorIntent {
  requestOpen: () => void;
}

let activePrSelectorIntent: ActivePrSelectorIntent | null = null;

export function setActivePrSelectorIntent(intent: ActivePrSelectorIntent | null) {
  activePrSelectorIntent = intent;
}

export function requestActivePrSelectorOpen() {
  activePrSelectorIntent?.requestOpen();
}
