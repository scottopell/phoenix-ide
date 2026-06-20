import type { McpServerStatus, SkillEntry, TaskEntry, WorkScopeInventory } from '../../api';
import type { FileItem } from '../../components/FileExplorer/FileTree';
import type { GroundingPanelFixtureData, GroundingPanelScenario, GroundingPanelScenarioId } from './types';

export const GROUNDING_PANEL_ROOT = '/Users/scottopell/dev/phoenix-ide/.phoenix/seed-worktrees/grounding-panel-qa';
export const GROUNDING_PANEL_CONVERSATION_ID = 'qa-grounding-conversation';
export const GROUNDING_PANEL_SCOPE_KEY = `worktree:${GROUNDING_PANEL_ROOT}`;

function file(name: string, path: string): FileItem {
  return { name, path, is_directory: false, viewer: { kind: 'text', category: name.endsWith('.md') ? 'markdown' : 'code' }, is_gitignored: false, modified_time: 1700000000 };
}

function dir(name: string, path: string): FileItem {
  return { name, path, is_directory: true, viewer: { kind: 'opaque' }, is_gitignored: false, modified_time: 1700000000 };
}

const files = new Map<string, FileItem[]>([
  [GROUNDING_PANEL_ROOT, [
    dir('crates', `${GROUNDING_PANEL_ROOT}/crates`),
    dir('ui', `${GROUNDING_PANEL_ROOT}/ui`),
    dir('specs', `${GROUNDING_PANEL_ROOT}/specs`),
    dir('tasks', `${GROUNDING_PANEL_ROOT}/tasks`),
    dir('.agents', `${GROUNDING_PANEL_ROOT}/.agents`),
    file('README-with-a-very-long-name-that-still-needs-to-truncate.md', `${GROUNDING_PANEL_ROOT}/README-with-a-very-long-name-that-still-needs-to-truncate.md`),
  ]],
  [`${GROUNDING_PANEL_ROOT}/crates`, [dir('phoenix-ide', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide`), dir('phoenix-tls', `${GROUNDING_PANEL_ROOT}/crates/phoenix-tls`)]],
  [`${GROUNDING_PANEL_ROOT}/crates/phoenix-ide`, [dir('src', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/src`), file('Cargo.toml', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/Cargo.toml`)]],
  [`${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/src`, [dir('runtime', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/src/runtime`), dir('state_machine', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/src/state_machine`), file('main.rs', `${GROUNDING_PANEL_ROOT}/crates/phoenix-ide/src/main.rs`)]],
  [`${GROUNDING_PANEL_ROOT}/ui`, [dir('src', `${GROUNDING_PANEL_ROOT}/ui/src`), file('package.json', `${GROUNDING_PANEL_ROOT}/ui/package.json`)]],
  [`${GROUNDING_PANEL_ROOT}/ui/src`, [dir('components', `${GROUNDING_PANEL_ROOT}/ui/src/components`), file('App.tsx', `${GROUNDING_PANEL_ROOT}/ui/src/App.tsx`)]],
  [`${GROUNDING_PANEL_ROOT}/tasks`, [
    file('22001-p2-in-progress--redesign-conversation-grounding-side-panel.md', `${GROUNDING_PANEL_ROOT}/tasks/22001-p2-in-progress--redesign-conversation-grounding-side-panel.md`),
    file('22002-p0-ready--fix-critical-mcp-auth-refresh-loop-with-long-slug.md', `${GROUNDING_PANEL_ROOT}/tasks/22002-p0-ready--fix-critical-mcp-auth-refresh-loop-with-long-slug.md`),
    file('22003-p1-blocked--blocked-on-github-token-scope-decision.md', `${GROUNDING_PANEL_ROOT}/tasks/22003-p1-blocked--blocked-on-github-token-scope-decision.md`),
    file('22004-p3-brainstorming--explore-grounding-panel-information-architecture.md', `${GROUNDING_PANEL_ROOT}/tasks/22004-p3-brainstorming--explore-grounding-panel-information-architecture.md`),
    file('22005-p4-done--archive-old-panel-screenshot-notes.md', `${GROUNDING_PANEL_ROOT}/tasks/22005-p4-done--archive-old-panel-screenshot-notes.md`),
    file('22006-p2-wont-do--replace-panel-with-floating-modal.md', `${GROUNDING_PANEL_ROOT}/tasks/22006-p2-wont-do--replace-panel-with-floating-modal.md`),
  ]],
  [`${GROUNDING_PANEL_ROOT}/.agents`, [dir('skills', `${GROUNDING_PANEL_ROOT}/.agents/skills`)]],
  [`${GROUNDING_PANEL_ROOT}/.agents/skills`, [dir('phoenix-perf-hunt', `${GROUNDING_PANEL_ROOT}/.agents/skills/phoenix-perf-hunt`)]],
]);

const mcp: McpServerStatus[] = [
  { name: 'github', state: 'ready', transport: 'http', auth: 'oauth', tool_count: 12, tools: ['list_prs', 'get_issue', 'create_review', 'merge_pull_request', 'search_code', 'get_workflow_run', 'rerun_workflow'], enabled: true },
  { name: 'filesystem-readonly-reference-server-with-long-name', state: 'ready', transport: 'stdio', auth: 'none', tool_count: 5, tools: ['read_file', 'list_directory', 'stat', 'search', 'checksum'], enabled: false },
  { name: 'linear', state: 'unauthorized', transport: 'http', auth: 'oauth', tool_count: 0, tools: [], enabled: true, pending_oauth_url: 'https://linear.app/oauth/authorize', auth_redirect_warning: 'OAuth redirect points at localhost; remote browser may not be able to complete this flow.' },
  { name: 'design-system', state: 'failed', transport: 'stdio', auth: 'static', tool_count: 0, tools: [], enabled: true, last_error: 'spawn ENOENT: design-system-mcp not found on PATH' },
];

const skills: SkillEntry[] = [
  { name: 'rust-dev', description: 'Rust workflow, test targeting, clippy triage, and ergonomic code review for backend changes.', argument_hint: '[crate-or-test-filter]', source: 'builtin', path: '/Users/scottopell/.phoenix-ide/builtin-skills/rust-dev/SKILL.md' },
  { name: 'agent-browser', description: 'Drive a real browser for screenshots, repro steps, and exploratory UI testing with deterministic evidence.', argument_hint: '<url> [goal]', source: '/Users/scottopell/.agents/skills', path: '/Users/scottopell/.agents/skills/agent-browser/SKILL.md' },
  { name: 'phoenix-perf-hunt', description: 'Profiles Phoenix React scenarios with raw samples and coordinates one focused performance attempt.', source: `${GROUNDING_PANEL_ROOT}/.agents/skills`, path: `${GROUNDING_PANEL_ROOT}/.agents/skills/phoenix-perf-hunt/SKILL.md` },
  { name: 'very-long-project-specific-skill-name-for-truncation-review', description: 'Project skill with a deliberately long description that should wrap or truncate consistently without pushing counts and status metadata off screen.', argument_hint: '--scenario <name> --evidence <path>', source: `${GROUNDING_PANEL_ROOT}/.claude/skills`, path: `${GROUNDING_PANEL_ROOT}/.claude/skills/very-long-project-specific-skill-name-for-truncation-review/SKILL.md` },
];

const tasks: TaskEntry[] = [
  { id: '22001', priority: 'p2', status: 'in-progress', slug: 'redesign-conversation-grounding-side-panel', path: `${GROUNDING_PANEL_ROOT}/tasks/22001-p2-in-progress--redesign-conversation-grounding-side-panel.md`, conversation_slug: 'grounding-panel-redesign' },
  { id: '22002', priority: 'p0', status: 'ready', slug: 'fix-critical-mcp-auth-refresh-loop-with-long-slug', path: `${GROUNDING_PANEL_ROOT}/tasks/22002-p0-ready--fix-critical-mcp-auth-refresh-loop-with-long-slug.md` },
  { id: '22003', priority: 'p1', status: 'blocked', slug: 'blocked-on-github-token-scope-decision', path: `${GROUNDING_PANEL_ROOT}/tasks/22003-p1-blocked--blocked-on-github-token-scope-decision.md`, conversation_slug: 'blocked-token-scope' },
  { id: '22004', priority: 'p3', status: 'brainstorming', slug: 'explore-grounding-panel-information-architecture', path: `${GROUNDING_PANEL_ROOT}/tasks/22004-p3-brainstorming--explore-grounding-panel-information-architecture.md` },
  { id: '22005', priority: 'p4', status: 'done', slug: 'archive-old-panel-screenshot-notes', path: `${GROUNDING_PANEL_ROOT}/tasks/22005-p4-done--archive-old-panel-screenshot-notes.md` },
  { id: '22006', priority: 'p2', status: 'wont-do', slug: 'replace-panel-with-floating-modal', path: `${GROUNDING_PANEL_ROOT}/tasks/22006-p2-wont-do--replace-panel-with-floating-modal.md` },
];

const workScope: WorkScopeInventory = {
  scope_key: GROUNDING_PANEL_SCOPE_KEY,
  bash: [
    { handle_id: 'b-1', label: 'vite dev server', cmd: 'pnpm --dir ui dev', state: 'running', pid: 42100, pgid: 42100, started_at: new Date(Date.now() - 75_000).toISOString(), output_bytes: 1_200_000 },
    { handle_id: 'b-2', label: 'stuck test cleanup', cmd: './dev.py check --lanes ui', state: 'kill_pending_kernel', pid: 42110, pgid: 42110, started_at: new Date(Date.now() - 185_000).toISOString(), output_bytes: 23_000 },
    { handle_id: 'b-3', label: 'seed fixture', cmd: './dev.py seed', state: 'tombstoned', started_at: new Date(Date.now() - 480_000).toISOString(), duration_ms: 34_000, exit_code: 0, output_bytes: 82_000 },
    { handle_id: 'b-4', label: 'failing lint', cmd: 'pnpm lint', state: 'tombstoned', started_at: new Date(Date.now() - 360_000).toISOString(), duration_ms: 19_000, exit_code: 2, output_bytes: 18_000 },
  ],
  tmux: { status: 'live' },
  browser: { state: 'live', idle_ms: 95_000 },
};

export const groundingPanelFixtureData: GroundingPanelFixtureData = {
  files,
  mcp,
  skills,
  tasks,
  workScope,
  taskDetailMarkdown: '# Task brief\n\nRedesign the conversation grounding side panel with consistent sections, attention states, and reproducible QA screenshots.\n\n## Notes\n\n- Keep data scoped to the active conversation.\n- Make current task and blocked work visible.\n- Preserve keyboard access for rows and detail navigation.\n\nLong markdown paragraph: this task contains enough prose to exercise wrapping inside the detail view without making the panel wider than the minimum useful width.',
  skillDetailMarkdown: '# Skill: fixture detail\n\nUse this skill when a grounding panel screenshot needs long markdown content, argument hints, and enough prose to validate scrolling.\n\n## Arguments\n\n`--scenario <name> --evidence <path>`\n\n## Guidance\n\nPrefer deterministic fixtures over ad-hoc data. Capture screenshots after async sections settle and inspect console output for render errors.',
};

const scenarioDefinitions = [
  { id: 'full-dark', title: 'Full / dark', kind: 'full', theme: 'dark', width: 360, collapsed: false },
  { id: 'full-light', title: 'Full / light', kind: 'full', theme: 'light', width: 360, collapsed: false },
  { id: 'empty-dark', title: 'Empty states', kind: 'empty', theme: 'dark', width: 360, collapsed: false },
  { id: 'errors-dark', title: 'Error states', kind: 'errors', theme: 'dark', width: 360, collapsed: false },
  { id: 'collapsed-dark', title: 'Collapsed rail', kind: 'collapsed', theme: 'dark', width: 360, collapsed: true },
  { id: 'narrow-dark', title: 'Narrow panel', kind: 'narrow', theme: 'dark', width: 248, collapsed: false },
  { id: 'skill-detail-dark', title: 'Selected skill detail', kind: 'skill-detail', theme: 'dark', width: 360, collapsed: false },
  { id: 'task-detail-dark', title: 'Selected task detail', kind: 'task-detail', theme: 'dark', width: 360, collapsed: false },
] as const;

export const groundingPanelScenarios: GroundingPanelScenario[] = scenarioDefinitions.map((scenario) => ({
  rootPath: GROUNDING_PANEL_ROOT,
  conversationId: GROUNDING_PANEL_CONVERSATION_ID,
  scopeKey: GROUNDING_PANEL_SCOPE_KEY,
  branchName: 'task-22001-redesign-conversation-grounding-side-panel',
  activeSlug: 'grounding-panel-redesign',
  ...scenario,
}));

export function getGroundingPanelScenario(id: string | null | undefined): GroundingPanelScenario {
  return groundingPanelScenarios.find((scenario) => scenario.id === id)
    ?? groundingPanelScenarios.find((scenario) => scenario.kind === id)
    ?? groundingPanelScenarios[0]!;
}

export function emptyWorkScope(): WorkScopeInventory {
  return { scope_key: GROUNDING_PANEL_SCOPE_KEY, bash: [], tmux: null, browser: null };
}

export type { GroundingPanelScenario, GroundingPanelScenarioId };
