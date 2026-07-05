import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'task-approval',
  readyAttribute: 'data-task-approval-fixture-ready',
  outDir: process.env.TASK_APPROVAL_QA_OUT ?? 'qa-artifacts/task-approval',
  viewport: { width: 390, height: 844 },
});
