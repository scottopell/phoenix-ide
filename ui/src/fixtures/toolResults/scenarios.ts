import type { Message, SubAgentResult, ToolResultContent } from '../../api';
import type { ToolResultsFixtureData, ToolResultsScenario } from './types';

const CONVERSATION_ID = 'fixture-tool-results';
const FILE_ROOT = '/repo/fixture-tool-results';
const WORK_SCOPE_KEY = 'fixture:tool-results';
const THEME = 'dark' as const;

function createdAt(sequenceId: number, seconds: string): string {
  return `2026-02-01T10:${String(sequenceId).padStart(2, '0')}:${seconds}.000Z`;
}

function userMessage(sequence_id: number, text: string): Message {
  return {
    message_id: `user-${sequence_id}`,
    conversation_id: CONVERSATION_ID,
    sequence_id,
    type: 'user',
    message_type: 'user',
    created_at: createdAt(sequence_id, '00'),
    content: { text },
    display_data: {},
  };
}

function agentMessage(sequence_id: number, blocks: Message['content']): Message {
  return {
    message_id: `agent-${sequence_id}`,
    conversation_id: CONVERSATION_ID,
    sequence_id,
    type: 'agent',
    message_type: 'agent',
    created_at: createdAt(sequence_id, '30'),
    content: blocks,
    display_data: {},
  };
}

function toolMessage(
  sequence_id: number,
  tool_use_id: string,
  content: string,
  options?: {
    is_error?: boolean;
    display_data?: Record<string, unknown> | null;
    images?: Array<{ data: string; media_type: string }>;
  },
): Message {
  const toolContent: ToolResultContent = {
    tool_use_id,
    content,
    is_error: options?.is_error ?? false,
  };
  if (options?.images) toolContent.images = options.images;
  return {
    message_id: `tool-${tool_use_id}`,
    conversation_id: CONVERSATION_ID,
    sequence_id,
    type: 'tool',
    message_type: 'tool',
    created_at: createdAt(sequence_id, '45'),
    content: toolContent,
    display_data: options?.display_data ?? {},
  };
}

const fixtureImageMediaType = 'image/svg+xml';
const fixtureImageData = 'PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI2NDAiIGhlaWdodD0iMjQwIiB2aWV3Qm94PSIwIDAgNjQwIDI0MCI+PHJlY3Qgd2lkdGg9IjY0MCIgaGVpZ2h0PSIyNDAiIHJ4PSIxOCIgZmlsbD0iIzExMTgyNyIvPjxyZWN0IHg9IjI0IiB5PSIyNCIgd2lkdGg9IjU5MiIgaGVpZ2h0PSIxOTIiIHJ4PSIxMiIgZmlsbD0iIzFmMjkzNyIgc3Ryb2tlPSIjNjBhNWZhIiBzdHJva2Utd2lkdGg9IjMiLz48Y2lyY2xlIGN4PSI1OCIgY3k9IjU4IiByPSI5IiBmaWxsPSIjMjJjNTVlIi8+PHRleHQgeD0iODIiIHk9IjY2IiBmaWxsPSIjZjlmYWZiIiBmb250LWZhbWlseT0ibW9ub3NwYWNlIiBmb250LXNpemU9IjI0Ij50b29sIHJlc3VsdCBpbWFnZSBmaXh0dXJlPC90ZXh0Pjx0ZXh0IHg9IjQ0IiB5PSIxMjQiIGZpbGw9IiM5M2M1ZmQiIGZvbnQtZmFtaWx5PSJtb25vc3BhY2UiIGZvbnQtc2l6ZT0iMTgiPnR5cGVkIGltYWdlcyDigKIgZGlzcGxheV9kYXRhIOKAoiBsZWdhY3kgSlNPTjwvdGV4dD48cGF0aCBkPSJNNDQgMTc0IEwxNTAgMTQyIEwyMzIgMTg1IEwzMzIgMTEyIEw0MzAgMTY2IEw1OTYgOTIiIGZpbGw9Im5vbmUiIHN0cm9rZT0iI2E3OGJmYSIgc3Ryb2tlLXdpZHRoPSI2Ii8+PC9zdmc+';
const longToolText = Array.from({ length: 20 }, (_, index) => `line ${index + 1}: deterministic long fixture output`).join('\n');
const veryLongToolText = Array.from({ length: 18 }, (_, index) => `capture ${index + 1}: browser profile raw text fallback sample`).join('\n');

function bashLines(lines: string[]) {
  return lines.map((bytes, index) => ({ offset: index + 1, bytes }));
}

const subagentResults: SubAgentResult[] = [
  {
    agent_id: 'subagent-audit-success',
    task: 'Audit structured tool-result coverage and confirm every family has a compact twin.',
    outcome: {
      type: 'success',
      result: 'Structured families now cover lifecycle, execution, discovery, media, profiling, and sub-agent summaries with deterministic payloads.',
    },
  },
  {
    agent_id: 'subagent-audit-failure',
    task: 'Re-run browser profile fixtures against an unavailable local server to validate blocked/error handling.',
    outcome: {
      type: 'failure',
      error: 'Fixture intentionally uses synthetic blocked/error payloads instead of a live browser session.',
      error_kind: 'fixture_data_only',
    },
  },
  {
    agent_id: 'subagent-audit-timeout',
    task: 'Compare compact density screenshots after every family update.',
    outcome: { type: 'timed_out' },
  },
];

const shellMessages: Message[] = [
  userMessage(1, 'Show the comprehensive tool-result transcript shell, including lifecycle states and specialized input summaries.'),
  agentMessage(2, [
    { type: 'text', text: 'This shell family is the transcript-level smoke test: prose, browser action summaries, lifecycle placeholders, and fallback rendering all appear in one conversation.' },
    { type: 'tool_use', id: 'shell-missing', name: 'browser_click', input: { selector: '[data-testid="missing-result"]', wait: true, timeout: '5s' } },
    { type: 'tool_use', id: 'shell-short', name: 'browser_navigate', input: { url: 'https://fixture.local/tool-results', timeout: '5s' } },
    { type: 'tool_use', id: 'shell-empty', name: 'browser_clear_console_logs', input: {} },
    { type: 'tool_use', id: 'shell-error', name: 'browser_eval', input: { expression: 'window.__fixture?.crash()', timeout: '5s', await: true } },
    { type: 'tool_use', id: 'shell-long', name: 'browser_type', input: { selector: '#query', text: 'render every structured tool result family', clear: true, timeout: '5s' } },
    { type: 'tool_use', id: 'shell-truncated', name: 'browser_key_press', input: { key: 'Enter', modifiers: ['ctrl'], method: 'cdp' } },
    { type: 'tool_use', id: 'shell-unknown', name: 'custom_fixture_tool', input: { payload: 'unknown renderer fallback', mode: 'raw' } },
  ]),
  toolMessage(3, 'shell-short', 'Navigation complete — fixture page ready.', {
    display_data: { duration_ms: 142 },
  }),
  toolMessage(4, 'shell-empty', '', {
    display_data: { duration_ms: 11 },
  }),
  toolMessage(5, 'shell-error', 'Evaluation failed: Cannot read properties of undefined (reading "crash")', {
    is_error: true,
    display_data: { duration_ms: 16 },
  }),
  toolMessage(6, 'shell-long', longToolText, {
    display_data: { duration_ms: 61 },
  }),
  toolMessage(7, 'shell-truncated', `${veryLongToolText}\n... (312 more chars)`, {
    display_data: { duration_ms: 22 },
  }),
  toolMessage(8, 'shell-unknown', '{"unexpected":true,"note":"unknown tool renderer falls back to generic short output"}', {
    display_data: { duration_ms: 5 },
  }),
  agentMessage(9, [
    { type: 'text', text: 'The earlier click is a finalized missing result. This last operation is intentionally still active so its elapsed treatment remains distinct.' },
    { type: 'tool_use', id: 'shell-pending', name: 'browser_wait_for_selector', input: { selector: '.fixture-ready', visible: true, timeout: '5s' } },
  ]),
];

const executionMessages: Message[] = [
  userMessage(1, 'Group execution-oriented tool results, including bash, tmux, patch, and skill states.'),
  agentMessage(2, [
    { type: 'text', text: 'Execution families emphasize status-rich structured payloads and legacy fallbacks.' },
    { type: 'tool_use', id: 'exec-skill-loaded', name: 'skill', input: { skill_name: 'agent-browser', args: 'open http://localhost:8042/tool-results' } },
    { type: 'tool_use', id: 'exec-skill-error', name: 'skill', input: { skill_name: 'phoenix-release', args: 'ship v9.9.9' } },
    { type: 'tool_use', id: 'exec-bash-running', name: 'bash', input: { op: 'run', cmd: 'pnpm vitest run ui/src/components/MessageComponents.test.tsx', wait_seconds: 0, label: 'fixture-vitest' }, display: 'pnpm vitest run ui/src/components/MessageComponents.test.tsx' },
    { type: 'tool_use', id: 'exec-bash-still', name: 'bash', input: { op: 'wait', handle: 'b-17', wait_seconds: 30 } },
    { type: 'tool_use', id: 'exec-bash-kill-pending', name: 'bash', input: { op: 'kill', handle: 'b-17', signal: 'TERM' } },
    { type: 'tool_use', id: 'exec-bash-exited', name: 'bash', input: { op: 'wait', handle: 'b-22', wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-bash-killed', name: 'bash', input: { op: 'wait', handle: 'b-23', wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-bash-tombstoned', name: 'bash', input: { op: 'peek', handle: 'b-24', lines: 20 } },
    { type: 'tool_use', id: 'exec-bash-error', name: 'bash', input: { op: 'peek', handle: 'b-missing', lines: 20 } },
    { type: 'tool_use', id: 'exec-bash-legacy', name: 'bash', input: { command: 'git status --short' }, display: 'git status --short' },
    { type: 'tool_use', id: 'exec-tmux-ok', name: 'tmux', input: { args: ['capture-pane', '-pt', 'fixture:0.0'], wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-tmux-stderr', name: 'tmux', input: { args: ['display-message', '-p', '#{pane_id}'], wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-tmux-truncated', name: 'tmux', input: { args: ['capture-pane', '-pt', 'fixture:0.1', '-S', '-200'], wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-tmux-error', name: 'tmux', input: { args: ['wait-for', '-S', 'missing-signal'], wait_seconds: 5 } },
    { type: 'tool_use', id: 'exec-tmux-plain', name: 'tmux', input: { pane: '%12', lines: 20 } },
    { type: 'tool_use', id: 'exec-patch-trivial', name: 'patch', input: { path: 'ui/src/components/MessageComponents.tsx', patches: [{ operation: 'replace', oldText: 'pending', newText: 'running' }] } },
    { type: 'tool_use', id: 'exec-patch-multi', name: 'patch', input: { path: 'ui/src/fixtures/toolResults/scenarios.ts', patches: [{ operation: 'insert_after', oldText: 'const x = 1;', newText: 'const y = 2;' }, { operation: 'insert_after', oldText: 'const y = 2;', newText: 'const z = 3;' }] } },
    { type: 'tool_use', id: 'exec-patch-error', name: 'patch', input: { path: 'ui/src/fixtures/toolResults/scenarios.ts', patches: [{ operation: 'replace', oldText: 'missing anchor', newText: 'x' }] } },
    { type: 'tool_use', id: 'exec-patch-legacy', name: 'patch', input: { path: 'README.md', patches: [{ operation: 'append_eof', newText: 'legacy diff only' }] } },
  ]),
  toolMessage(3, 'exec-skill-loaded', 'Base directory for this skill: /repo/fixture-tool-results/.agents/skills/agent-browser\n# Browser Automation with agent-browser\nOpen a URL, take a screenshot, and inspect the DOM with deterministic browser helpers.', {
    display_data: { duration_ms: 118 },
  }),
  toolMessage(4, 'exec-skill-error', 'Skill not found: phoenix-release is unavailable in this test environment.', {
    is_error: true,
    display_data: { duration_ms: 7 },
  }),
  toolMessage(5, 'exec-bash-running', JSON.stringify({ status: 'running', handle: 'b-17', label: 'fixture-vitest', lines: bashLines([' RUN  v2.0.0 /repo/fixture-tool-results/ui', ' waiting for file watcher...']) }), {
    display_data: { duration_ms: 49 },
  }),
  toolMessage(6, 'exec-bash-still', JSON.stringify({ status: 'still_running', handle: 'b-17', label: 'fixture-vitest', waited_ms: 30000, lines: bashLines([' RUN  v2.0.0 /repo/fixture-tool-results/ui', ' 68 tests collected']) }), {
    display_data: { duration_ms: 30005 },
  }),
  toolMessage(7, 'exec-bash-kill-pending', JSON.stringify({ status: 'kill_pending_kernel', handle: 'b-17', kill_signal_sent: 'TERM', lines: bashLines([' node_modules/.bin/vitest still flushing coverage']) }), {
    display_data: { duration_ms: 30122 },
  }),
  toolMessage(8, 'exec-bash-exited', JSON.stringify({ status: 'exited', exit_code: 0, duration_ms: 1840, lines: bashLines([' ✓ src/components/MessageComponents.test.tsx (68 tests)', ' Test Files  1 passed']) }), {
    display_data: { duration_ms: 1840 },
  }),
  toolMessage(9, 'exec-bash-killed', JSON.stringify({ status: 'killed', exit_code: 137, signal_number: 9, duration_ms: 901, lines: bashLines([' process killed after timeout']) }), {
    display_data: { duration_ms: 901 },
  }),
  toolMessage(10, 'exec-bash-tombstoned', JSON.stringify({ status: 'tombstoned', final_cause: 'exited normally', exit_code: 0, handle: 'b-24', lines: bashLines([' archived output retained for inspection']) }), {
    display_data: { duration_ms: 12 },
  }),
  toolMessage(11, 'exec-bash-error', JSON.stringify({ error: 'handle_not_found', error_message: 'Handle b-missing does not exist in this work scope.', hint: 'A Phoenix restart clears transient bash handles.' }), {
    is_error: true,
    display_data: { duration_ms: 3 },
  }),
  toolMessage(12, 'exec-bash-legacy', ' M ui/src/components/MessageComponents.tsx\n?? ui/src/fixtures/toolResults/scenarios.ts', {
    display_data: { duration_ms: 14 },
  }),
  toolMessage(13, 'exec-tmux-ok', JSON.stringify({ status: 'ok', exit_code: 0, duration_ms: 112, stdout: 'pane %12\ntool results fixture live preview\nwaiting for diff refresh', stderr: '', truncated: false }), {
    display_data: { duration_ms: 112 },
  }),
  toolMessage(14, 'exec-tmux-stderr', JSON.stringify({ status: 'ok', exit_code: 0, duration_ms: 22, stdout: 'pane_id=%12', stderr: 'warning: pane title omitted in detached session', truncated: false }), {
    display_data: { duration_ms: 22 },
  }),
  toolMessage(15, 'exec-tmux-truncated', JSON.stringify({ status: 'ok', exit_code: 0, duration_ms: 74, stdout: 'first line\nsecond line\nthird line', stderr: '', truncated: true }), {
    display_data: { duration_ms: 74 },
  }),
  toolMessage(16, 'exec-tmux-error', JSON.stringify({ error: 'tmux_wait_failed', message: 'tmux wait-for exited with status 1: no such signal' }), {
    is_error: true,
    display_data: { duration_ms: 17 },
  }),
  toolMessage(17, 'exec-tmux-plain', 'legacy tmux capture output without typed stdout/stderr envelope', {
    display_data: { duration_ms: 5 },
  }),
  toolMessage(18, 'exec-patch-trivial', 'Applied patch successfully', {
    display_data: {
      duration_ms: 28,
      diff: ['--- a/ui/src/components/MessageComponents.tsx', '+++ b/ui/src/components/MessageComponents.tsx', '@@', '-pending', '+running'].join('\n'),
    },
  }),
  toolMessage(19, 'exec-patch-multi', 'Applied patch successfully', {
    display_data: {
      duration_ms: 151,
      diff: ['--- a/ui/src/fixtures/toolResults/scenarios.ts', '+++ b/ui/src/fixtures/toolResults/scenarios.ts', '@@', '+const y = 2;', '@@', '+const z = 3;'].join('\n'),
    },
  }),
  toolMessage(20, 'exec-patch-error', 'Patch failed: oldText not found exactly once in ui/src/fixtures/toolResults/scenarios.ts', {
    is_error: true,
    display_data: { duration_ms: 19 },
  }),
  toolMessage(21, 'exec-patch-legacy', ['--- a/README.md', '+++ b/README.md', '@@', '+legacy diff only'].join('\n'), {
    display_data: { duration_ms: 13 },
  }),
  agentMessage(22, [
    { type: 'text', text: 'This family deliberately mixes typed payloads, plain-text legacy rows, and explicit error envelopes so execution affordances can be reviewed without a live backend.' },
  ]),
];

const discoveryMessages: Message[] = [
  userMessage(1, 'Show search-style discovery results, including empty and raw fallbacks.'),
  agentMessage(2, [
    { type: 'text', text: 'Discovery families compare structured search renderers against their fallback modes.' },
    { type: 'tool_use', id: 'discover-search-structured', name: 'search', input: { pattern: 'ToolUseBlock|BrowserProfileResponseView', path: 'ui/src/components', include: '*.tsx' } },
    { type: 'tool_use', id: 'discover-search-empty', name: 'search', input: { pattern: 'no_such_fixture_token', path: 'ui/src/fixtures', include: '*.ts' } },
    { type: 'tool_use', id: 'discover-search-raw', name: 'search', input: { pattern: 'raw fallback fixture', path: 'ui/src/components', include: '*.tsx' } },
    { type: 'tool_use', id: 'discover-keyword-structured', name: 'keyword_search', input: { query: 'tool result fixture shell scenarios structural tests', search_terms: ['tool result fixture', 'structural pairing coverage', 'real MessageList AgentMessage'] } },
    { type: 'tool_use', id: 'discover-keyword-empty', name: 'keyword_search', input: { query: 'missing thing', search_terms: ['missing thing'] } },
    { type: 'tool_use', id: 'discover-keyword-raw', name: 'keyword_search', input: { query: 'llm unavailable raw fallback', search_terms: ['llm unavailable', 'raw fallback'] } },
    { type: 'tool_use', id: 'discover-read', name: 'read_file', input: { path: 'ui/src/fixtures/toolResults/scenarios.ts', offset: 1, limit: 12 } },
  ]),
  toolMessage(3, 'discover-search-structured', [
    'ui/src/components/MessageComponents.tsx:1736:function ToolUseBlockImpl({ block, result, onOpenFile, workScopeKey, toolStartedAtMs, showMissingResult }: ToolUseBlockProps) {',
    'ui/src/components/BrowserProfileResponseView.tsx:22:export const STRUCTURED_PROFILE_ACTIONS = new Set([',
    '[Results limited to 50 matches. Use a more specific pattern or path to narrow results.]',
  ].join('\n')),
  toolMessage(4, 'discover-search-empty', 'No matches found.'),
  toolMessage(5, 'discover-search-raw', 'walk warning: permission denied while traversing ../private-fixtures'),
  toolMessage(6, 'discover-keyword-structured', [
    'ui/src/components/MessageComponents.tsx: Specialized tool-result renderers branch by tool name so transcripts stay readable without exposing raw JSON by default.',
    'ui/src/fixtures/toolResults/scenarios.test.ts: Structural fixture tests assert every family appears in full and compact density with meaningful pairing coverage.',
    'ui/src/fixtures/toolResults/renderFixture.tsx: The real MessageList shell is wrapped in deterministic story scaffolding for screenshot capture.',
  ].join('\n')),
  toolMessage(7, 'discover-keyword-empty', 'No relevant files found for the given search terms.'),
  toolMessage(8, 'discover-keyword-raw', [
    'ui/src/components/MessageComponents.tsx:1736:function ToolUseBlockImpl({ block, result, onOpenFile, workScopeKey, toolStartedAtMs, showMissingResult }: ToolUseBlockProps) {',
    '--',
    'ui/src/components/BrowserProfileResponseView.tsx-22-export const STRUCTURED_PROFILE_ACTIONS = new Set([',
    'ui/src/components/BrowserProfileResponseView.tsx:23:  "run_scenario",',
  ].join('\n')),
  toolMessage(9, 'discover-read', Array.from({ length: 12 }, (_, index) => `${index + 1}\tconst deterministicFixtureLine${index + 1} = true;`).join('\n')),
  agentMessage(10, [
    { type: 'text', text: 'read_file stays in this family because it helps reviewers correlate search hits with the deterministic fixture source lines they come from.' },
  ]),
];

const mediaMessages: Message[] = [
  userMessage(1, 'Show media renderers, console logs, and image payload compatibility fallbacks.'),
  agentMessage(2, [
    { type: 'text', text: 'Media families cover typed images, legacy display_data images, malformed image fallbacks, and browser console log parsing states.' },
    { type: 'tool_use', id: 'media-read-image-typed', name: 'read_image', input: { path: '/tmp/fixture-typed.png', timeout: '5s' } },
    { type: 'tool_use', id: 'media-screenshot-display', name: 'browser_take_screenshot', input: { selector: '.fixture-stage', timeout: '5s' } },
    { type: 'tool_use', id: 'media-read-image-legacy', name: 'read_image', input: { path: '/tmp/fixture-legacy.png', timeout: '5s' } },
    { type: 'tool_use', id: 'media-read-image-malformed', name: 'read_image', input: { path: '/tmp/fixture-malformed.png', timeout: '5s' } },
    { type: 'tool_use', id: 'media-console-empty', name: 'browser_recent_console_logs', input: { limit: 10 } },
    { type: 'tool_use', id: 'media-console-structured', name: 'browser_recent_console_logs', input: { limit: 4 } },
    { type: 'tool_use', id: 'media-console-pointer', name: 'browser_recent_console_logs', input: { limit: 200 } },
    { type: 'tool_use', id: 'media-console-unparseable', name: 'browser_recent_console_logs', input: { limit: 20 } },
  ]),
  toolMessage(3, 'media-read-image-typed', 'Loaded image fixture from typed channel.', {
    display_data: { duration_ms: 90 },
    images: [{ data: fixtureImageData, media_type: fixtureImageMediaType }],
  }),
  toolMessage(4, 'media-screenshot-display', 'Screenshot saved to /tmp/fixture-shot.png', {
    display_data: { type: 'image', media_type: fixtureImageMediaType, data: fixtureImageData, duration_ms: 140 },
  }),
  toolMessage(5, 'media-read-image-legacy', JSON.stringify({ type: 'image', media_type: fixtureImageMediaType, data: fixtureImageData }), {
    display_data: { duration_ms: 80 },
  }),
  toolMessage(6, 'media-read-image-malformed', JSON.stringify({ type: 'image', media_type: 'image/png' }), {
    display_data: { duration_ms: 81 },
  }),
  toolMessage(7, 'media-console-empty', '[]', {
    display_data: { duration_ms: 18 },
  }),
  toolMessage(8, 'media-console-structured', JSON.stringify([
    { level: 'info', text: 'fixture shell mounted' },
    { level: 'warning', text: 'compact density hides some detail until expanded' },
    { level: 'error', text: 'synthetic renderer warning for screenshot validation' },
  ]), {
    display_data: { duration_ms: 25 },
  }),
  toolMessage(9, 'media-console-pointer', 'Logs written to /tmp/phoenix-console-fixture.log', {
    display_data: { duration_ms: 30 },
  }),
  toolMessage(10, 'media-console-unparseable', '{bad json console payload', {
    display_data: { duration_ms: 21 },
  }),
  agentMessage(11, [
    { type: 'text', text: 'The malformed read_image row is intentionally missing its base64 payload so the UI falls back to generic text instead of fabricating an image preview.' },
  ]),
];

const profilingMessages: Message[] = [
  userMessage(1, 'Show browser_profile coverage for completed, blocked, error, missing, generic, and every structured action.'),
  agentMessage(2, [
    { type: 'text', text: 'Profiling families group all six structured browser_profile actions plus blocked/error/missing/generic fallbacks.' },
    { type: 'tool_use', id: 'profile-run-completed', name: 'browser_profile', input: { action: 'run_scenario', runs: 3, warmup: 1, throttle_rate: 4, steps: [{ kind: 'wait_selector', selector: '.fixture-ready' }, { kind: 'click', selector: '#run' }] } },
    { type: 'tool_use', id: 'profile-run-blocked', name: 'browser_profile', input: { action: 'run_scenario', runs: 2, warmup: 1, throttle_rate: 4, steps: [{ kind: 'wait_selector', selector: '.never-appears' }] } },
    { type: 'tool_use', id: 'profile-run-error', name: 'browser_profile', input: { action: 'run_scenario', runs: 2, warmup: 1, throttle_rate: 4, reset: 'none' } },
    { type: 'tool_use', id: 'profile-run-missing', name: 'browser_profile', input: { action: 'run_scenario', runs: 1, warmup: 0, steps: [] } },
    { type: 'tool_use', id: 'profile-metrics', name: 'browser_profile', input: { action: 'metrics' } },
    { type: 'tool_use', id: 'profile-cpu-stop', name: 'browser_profile', input: { action: 'cpu_stop' } },
    { type: 'tool_use', id: 'profile-cpu-summary', name: 'browser_profile', input: { action: 'cpu_summary', path: '/tmp/phoenix-cpu-profile-fixture.json' } },
    { type: 'tool_use', id: 'profile-trace', name: 'browser_profile', input: { action: 'trace_stop' } },
    { type: 'tool_use', id: 'profile-heap', name: 'browser_profile', input: { action: 'heap_snapshot', baseline: '/tmp/phoenix-heap-before.heapsnapshot' } },
    { type: 'tool_use', id: 'profile-generic', name: 'browser_profile', input: { action: 'why_render' } },
  ]),
  toolMessage(3, 'profile-run-completed', 'Scenario completed', {
    display_data: {
      outcome: 'completed',
      requested_runs: 3,
      warmup: 1,
      methodology_warnings: ['CPU throttle omitted in one baseline capture; compare raw samples before publishing results.'],
      raw_samples: [
        { run_index: 0, script_ms: 12.5, long_tasks: 1, wall_ms: 100.5, dom_nodes: 1500, gc_ran: true, js_heap_used: 2000000, react_status: 'absent', react_commits: null, react_actual_ms: null },
        { run_index: 1, script_ms: 13.1, long_tasks: 1, wall_ms: 101.0, dom_nodes: 1502, gc_ran: true, js_heap_used: 2100000, react_status: 'absent', react_commits: null, react_actual_ms: null },
        { run_index: 2, script_ms: 11.8, long_tasks: 0, wall_ms: 99.2, dom_nodes: 1498, gc_ran: true, js_heap_used: 2050000, react_status: 'absent', react_commits: null, react_actual_ms: null },
      ],
    },
  }),
  toolMessage(4, 'profile-run-blocked', 'Scenario blocked waiting for readiness selector.', {
    is_error: true,
    display_data: {
      outcome: 'blocked',
      requested_runs: 2,
      warmup: 1,
      blocked_step: 'wait_selector(.never-appears)',
      methodology_warnings: ['The readiness selector never appeared; measured steps did not start.'],
      raw_samples: [],
    },
  }),
  toolMessage(5, 'profile-run-error', 'browser_profile run_scenario failed: page crashed before measurement window opened.', {
    is_error: true,
    display_data: { duration_ms: 44 },
  }),
  toolMessage(6, 'profile-metrics', 'Metrics snapshot', {
    display_data: {
      metrics: {
        ScriptDuration: 0.123,
        JSHeapUsedSize: 2500000,
        Nodes: 1234,
      },
    },
  }),
  toolMessage(7, 'profile-cpu-stop', 'CPU profile captured', {
    display_data: {
      cpu_summary: {
        path: '/tmp/phoenix-cpu-profile-fixture.json',
        hitcount_fallback: false,
        total: 401.5,
        top_by_self: [
          { label: 'busyLoop  app.js:42', value: 380.0, percent: 94.6 },
          { label: 'sqrt  (native)', value: 21.5, percent: 5.4 },
        ],
        top_by_total: [
          { label: '(root)', value: 401.5, percent: 100.0 },
          { label: 'busyLoop  app.js:42', value: 380.0, percent: 94.6 },
        ],
      },
    },
  }),
  toolMessage(8, 'profile-cpu-summary', 'CPU summary loaded from saved profile', {
    display_data: {
      cpu_summary: {
        path: '/tmp/phoenix-cpu-profile-fixture.json',
        hitcount_fallback: true,
        total: 812.2,
        top_by_self: [
          { label: 'hydrateFixture  profile.js:87', value: 510.3, percent: 62.8 },
          { label: 'JSON.parse  (native)', value: 88.2, percent: 10.9 },
        ],
        top_by_total: [
          { label: '(root)', value: 812.2, percent: 100.0 },
          { label: 'hydrateFixture  profile.js:87', value: 620.7, percent: 76.4 },
        ],
      },
    },
  }),
  toolMessage(9, 'profile-trace', 'Trace saved', {
    display_data: {
      trace: {
        path: '/tmp/phoenix-trace-fixture.json',
        event_count: 5234,
        long_task_count: 2,
        long_task_total_ms: 145.7,
        long_tasks: [
          { name: 'RunTask', ms: 100.2 },
          { name: 'ParseHTML', ms: 45.5 },
        ],
        timed_out: false,
      },
    },
  }),
  toolMessage(10, 'profile-heap', 'Heap diff ready', {
    display_data: {
      baseline: '/tmp/phoenix-heap-before.heapsnapshot',
      post: '/tmp/phoenix-heap-after.heapsnapshot',
      node_count_delta: 587,
      self_size_delta_bytes: 628736,
      retained_size_approximate: true,
      detached_dom_nodes: { baseline: 12, post: 47 },
    },
  }),
  toolMessage(11, 'profile-generic', veryLongToolText, {
    display_data: { duration_ms: 58 },
  }),
  agentMessage(12, [
    { type: 'text', text: 'One run_scenario tool use is left intentionally without a paired result so the transcript still covers the missing-result state for browser_profile without inventing fake structured data.' },
  ]),
];

const subagentMessages: Message[] = [
  userMessage(1, 'Show the spawn_agents summary renderer with corrected task-shaped input and deterministic outcomes.'),
  agentMessage(2, [
    { type: 'text', text: 'The spawn_agents fixture uses the approved tasks-shaped input instead of the old agents-count payload, then renders success, failure, and timeout outcomes.' },
    {
      type: 'tool_use',
      id: 'subagents-spawn',
      name: 'spawn_agents',
      input: {
        tasks: subagentResults.map((result) => ({ task: result.task })),
      },
    },
  ]),
  toolMessage(3, 'subagents-spawn', 'Spawned 3 agents', {
    display_data: { type: 'subagent_summary', results: subagentResults },
  }),
  agentMessage(4, [
    { type: 'text', text: 'Live running-state cards are not representable in persisted tool-result fixture data because the summary block only appears once outcomes exist; the dedicated conversation runtime covers the in-flight variant.' },
  ]),
];

export const toolResultsScenarios = [
  { id: 'shell-full', title: 'Shell — full', description: 'Transcript shell with lifecycle states, browser action summaries, and fallback rendering in full density.', density: 'full', family: 'shell' },
  { id: 'shell-compact', title: 'Shell — compact', description: 'The same transcript shell rendered with compact density.', density: 'compact', family: 'shell' },
  { id: 'execution-full', title: 'Execution renderers — full', description: 'Skill, bash, tmux, and patch states in full density.', density: 'full', family: 'execution' },
  { id: 'execution-compact', title: 'Execution renderers — compact', description: 'Execution-oriented renderers in compact density.', density: 'compact', family: 'execution' },
  { id: 'discovery-full', title: 'Discovery renderers — full', description: 'search, keyword_search, and read_file grouped with empty/raw fallbacks.', density: 'full', family: 'discovery' },
  { id: 'discovery-compact', title: 'Discovery renderers — compact', description: 'Discovery renderers in compact density.', density: 'compact', family: 'discovery' },
  { id: 'media-full', title: 'Media renderers — full', description: 'Image payloads and browser console log parsing states.', density: 'full', family: 'media' },
  { id: 'media-compact', title: 'Media renderers — compact', description: 'Media-oriented renderers in compact density.', density: 'compact', family: 'media' },
  { id: 'profiling-full', title: 'Profiling renderers — full', description: 'browser_profile structured actions plus blocked/error/missing/generic states.', density: 'full', family: 'profiling' },
  { id: 'profiling-compact', title: 'Profiling renderers — compact', description: 'browser_profile family with compact transcript chrome.', density: 'compact', family: 'profiling' },
  { id: 'subagents-full', title: 'Sub-agent summary — full', description: 'Persistent spawn_agents summary block in full density.', density: 'full', family: 'subagents' },
  { id: 'subagents-compact', title: 'Sub-agent summary — compact', description: 'Persistent spawn_agents summary block in compact density.', density: 'compact', family: 'subagents' },
] as const satisfies readonly ToolResultsScenario[];

export type ToolResultsScenarioId = (typeof toolResultsScenarios)[number]['id'];

export function getToolResultsScenario(id: ToolResultsScenarioId): ToolResultsScenario {
  const scenario = toolResultsScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown tool results scenario: ${id}`);
  return scenario;
}

const messagesByFamily = {
  shell: shellMessages,
  execution: executionMessages,
  discovery: discoveryMessages,
  media: mediaMessages,
  profiling: profilingMessages,
  subagents: subagentMessages,
} as const satisfies Record<ToolResultsScenario['family'], Message[]>;

export function toolResultsFixtureData(scenario: ToolResultsScenario): ToolResultsFixtureData {
  return {
    conversationId: `${CONVERSATION_ID}-${scenario.id}`,
    slug: `${CONVERSATION_ID}-${scenario.id}`,
    theme: THEME,
    density: scenario.density,
    filePathRootDir: FILE_ROOT,
    workScopeKey: WORK_SCOPE_KEY,
    messages: messagesByFamily[scenario.family],
    pendingMessages: [],
    convState: scenario.family === 'shell'
      ? {
          type: 'tool_executing',
          current_tool: { id: 'shell-pending', name: 'browser_wait_for_selector', input: { selector: '.fixture-ready' } },
          remaining_tools: [],
        }
      : { type: 'idle' },
  };
}
