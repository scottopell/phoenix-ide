import type { ComposerQuickAction } from '../components/InputArea';

export const COORDINATOR_BRIEFING_PROMPT = 'Check in & drive: check commissioned work, including idle owners. Leave verified healthy progress alone; diagnose blocked or waiting states before deciding no action is needed. Take the smallest recovery step through existing owners within verified standing authority and verify work resumed. Progress elsewhere does not clear a blocker. Preserve prior decisions, pauses and gates; check-ins add no authority. Before replaying answers or questions, reconcile later decisions and completed work. Verify unanswered choices still matter; advance independent authorized work, then use ask_user_question with context and a recommendation. Assume I missed intervening messages: lead with needed input, then what changed, why it matters and what happens next. Explain the work in plain English, not internal process labels. Link supporting detail; distinguish requests from verified outcomes. No fixed template, redundant nudges or indefinite polling.';

export const COORDINATOR_QUICK_ACTION: ComposerQuickAction = {
  label: 'Check in & drive',
  compactLabel: 'Check in & drive',
  prompt: COORDINATOR_BRIEFING_PROMPT,
};
