import type { ComposerQuickAction } from '../components/InputArea';

export const COORDINATOR_BRIEFING_PROMPT = 'Get fresh current-work facts through query_database. Give me a concise check-in: decisions or blockers needing my attention first, then work actively progressing. Use current continuation rows and inspect recent messages only where needed; distinguish observed facts from uncertainty and cite the evidence. Do not send messages, change anything, or start a polling loop.';

export const COORDINATOR_QUICK_ACTION: ComposerQuickAction = {
  label: 'Brief me on current work',
  compactLabel: 'Brief me',
  prompt: COORDINATOR_BRIEFING_PROMPT,
};
