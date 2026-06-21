import { runSurfaceCapture } from './capture-ladle-surface.mjs';

// Surface config for the grounding-panel QA capture. The engine (Ladle boot,
// manifest-driven story discovery, settled-DOM wait, console-error gating,
// teardown) lives in capture-ladle-surface.mjs and is shared with other
// `./dev.py qa <surface>` workflows.
runSurfaceCapture({
  surface: 'grounding-panel',
  readyAttribute: 'data-grounding-fixture-ready',
  outDir: process.env.GROUNDING_PANEL_QA_OUT ?? 'qa-artifacts/grounding-panel',
  expectedConsoleErrors: new Map([
    ['errors-dark', [
      'Failed to list tasks',
      'Failed to fetch task counts',
      'Failed to list skills',
      'Failed to fetch MCP status',
      'Failed to load work scope',
      'Failed to list files',
    ]],
  ]),
});
