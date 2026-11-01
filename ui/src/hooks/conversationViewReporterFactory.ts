import { ViewJourneyReporter } from './conversationViewTelemetry';

export function createViewJourneyReporter(): ViewJourneyReporter {
  return new ViewJourneyReporter({ send: (input, init) => fetch(input, init) });
}
