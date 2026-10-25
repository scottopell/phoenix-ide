import type { ComposerQuickAction } from '../components/InputArea';

export const COORDINATOR_BRIEFING_PROMPT = 'Check in & drive: make a bounded whole-stack pass over commissioned work, including idle owners. Use fresh facts and verified standing authority to move routine next steps through existing owners, resuming them within already-approved scope rather than only recommending action. Respect later pauses and real decision gates; advance independent work before asking me a genuine choice through ask_user_question. Then tell me in plain English what problem each workstream solves, what you did, and where delivery stands or goes next. Group work with links; use statuses or rough progress only when helpful. Mention process or evidence limits only when they change my understanding or decision. Do not wait indefinitely for owner replies or infer completion from a message send.';

export const COORDINATOR_QUICK_ACTION: ComposerQuickAction = {
  label: 'Check in & drive',
  compactLabel: 'Check in & drive',
  prompt: COORDINATOR_BRIEFING_PROMPT,
};
