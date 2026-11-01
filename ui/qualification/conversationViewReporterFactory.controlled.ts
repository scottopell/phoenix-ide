import { ViewJourneyReporter } from '../src/hooks/conversationViewTelemetry';

export function createViewJourneyReporter(): ViewJourneyReporter {
  return new ViewJourneyReporter({ sampleRate: 1, send: (input, init) => fetch(input, init) });
}
