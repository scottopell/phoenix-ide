import type { ComposerQuickAction } from '../components/InputArea';

export const COORDINATOR_BRIEFING_PROMPT = 'Check in & drive: make a bounded fresh pass over commissioned work, including idle owners, but intervene selectively. Leave healthy progressing work alone; do not nudge, replan, or reassign merely to show activity, and do not assume idle owners are stalled. Follow through on routine work through existing owners within verified standing authority. Repeated check-ins do not expand authority or reset prior decisions, pauses or gates. Recover unanswered user decisions from history and verify each is still unresolved and material. Advance independent authorized work first, then ask genuine choices through ask_user_question with context and a recommendation. Brief me first on what still needs my input, then material changes: the problem, what you did, and where delivery goes next. Use plain English and useful links, not a fixed report template. Mention delivery or evidence limits only when material; a message send is not completion. Do not wait indefinitely for owner replies.';

export const COORDINATOR_QUICK_ACTION: ComposerQuickAction = {
  label: 'Check in & drive',
  compactLabel: 'Check in & drive',
  prompt: COORDINATOR_BRIEFING_PROMPT,
};
