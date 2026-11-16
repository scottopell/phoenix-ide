import { captureQuestionFocus } from './capture-focus-regressions.mjs';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'conversation-panel',
  readyAttribute: 'data-conversation-panel-fixture-ready',
  outDir: process.env.CONVERSATION_PANEL_QA_OUT ?? 'qa-artifacts/conversation-panel',
  viewport: { width: 520, height: 900 },
  captureStory: async context => {
    if (context.id === 'question-focus') await captureQuestionFocus(context);
  },
});
