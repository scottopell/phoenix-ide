import type { ComposerQuickAction } from '../components/InputArea';

export const COORDINATOR_BRIEFING_PROMPT = 'Review the current deterministic work snapshot. Succinctly summarize what needs my attention now, what is actively progressing, and what may be stalled. Inspect conversation history only where needed to support the summary. Do not send messages or change anything.';

export const COORDINATOR_QUICK_ACTION: ComposerQuickAction = {
  label: 'Brief me on current work',
  compactLabel: 'Brief me',
  prompt: COORDINATOR_BRIEFING_PROMPT,
  context: 'Current work context is attached to each Coordinator message.',
};
