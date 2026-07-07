import { runSurfaceCapture } from './capture-ladle-surface.mjs';

const width = Number(process.env.MESSAGE_LIST_QA_WIDTH ?? 960);
const height = Number(process.env.MESSAGE_LIST_QA_HEIGHT ?? 900);

runSurfaceCapture({
  surface: 'message-list',
  readyAttribute: 'data-message-list-fixture-ready',
  outDir: process.env.MESSAGE_LIST_QA_OUT ?? 'qa-artifacts/message-list',
  viewport: { width, height },
});
