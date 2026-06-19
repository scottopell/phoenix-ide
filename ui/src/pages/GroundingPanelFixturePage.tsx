import { useEffect, useMemo, useState } from 'react';
import { FileExplorerPanel, FileExplorerProvider } from '../components/FileExplorer';
import type { FileItem } from '../components/FileExplorer/FileTree';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import type { McpServerStatus, SkillEntry, TaskEntry, WorkScopeInventory } from '../api';
import '../index.css';

const ROOT = '/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/a4b3167f-8480-4aac-84c0-1617fe37692b';
const CONV_ID = 'qa-grounding-conversation';
const SCOPE_KEY = `worktree:${ROOT}`;

type Scenario = 'full' | 'empty' | 'errors' | 'collapsed' | 'skill-detail' | 'task-detail' | 'narrow';

function file(name: string, path: string): FileItem {
  return { name, path, is_directory: false, viewer: { kind: 'text', category: name.endsWith('.md') ? 'markdown' : 'code' }, is_gitignored: false, modified_time: 1700000000 };
}

function dir(name: string, path: string): FileItem {
  return { name, path, is_directory: true, viewer: { kind: 'opaque' }, is_gitignored: false, modified_time: 1700000000 };
}

const FILES = new Map<string, FileItem[]>([
  [ROOT, [dir('crates', `${ROOT}/crates`), dir('ui', `${ROOT}/ui`), dir('specs', `${ROOT}/specs`), dir('tasks', `${ROOT}/tasks`), file('README-with-a-very-long-name-that-still-needs-to-truncate.md', `${ROOT}/README-with-a-very-long-name-that-still-needs-to-truncate.md`)]],
  [`${ROOT}/crates`, [dir('phoenix-ide', `${ROOT}/crates/phoenix-ide`), dir('phoenix-tls', `${ROOT}/crates/phoenix-tls`)]],
  [`${ROOT}/crates/phoenix-ide`, [dir('src', `${ROOT}/crates/phoenix-ide/src`), file('Cargo.toml', `${ROOT}/crates/phoenix-ide/Cargo.toml`)]],
  [`${ROOT}/crates/phoenix-ide/src`, [dir('runtime', `${ROOT}/crates/phoenix-ide/src/runtime`), dir('state_machine', `${ROOT}/crates/phoenix-ide/src/state_machine`), file('main.rs', `${ROOT}/crates/phoenix-ide/src/main.rs`)]],
  [`${ROOT}/ui`, [dir('src', `${ROOT}/ui/src`), file('package.json', `${ROOT}/ui/package.json`)]],
  [`${ROOT}/ui/src`, [dir('components', `${ROOT}/ui/src/components`), file('App.tsx', `${ROOT}/ui/src/App.tsx`)]],
  [`${ROOT}/tasks`, [file('22001-p2-in-progress--redesign-conversation-grounding-side-panel.md', `${ROOT}/tasks/22001-p2-in-progress--redesign-conversation-grounding-side-panel.md`)]],
]);

const MCP: McpServerStatus[] = [
  { name: 'github', state: 'ready', transport: 'http', auth: 'oauth', tool_count: 12, tools: ['list_prs', 'get_issue', 'create_review', 'merge_pull_request', 'search_code', 'get_workflow_run', 'rerun_workflow'], enabled: true },
  { name: 'filesystem-readonly-reference-server-with-long-name', state: 'ready', transport: 'stdio', auth: 'none', tool_count: 5, tools: ['read_file', 'list_directory', 'stat', 'search', 'checksum'], enabled: false },
  { name: 'linear', state: 'unauthorized', transport: 'http', auth: 'oauth', tool_count: 0, tools: [], enabled: true, pending_oauth_url: 'https://linear.app/oauth/authorize', auth_redirect_warning: 'OAuth redirect points at localhost; remote browser may not be able to complete this flow.' },
  { name: 'design-system', state: 'failed', transport: 'stdio', auth: 'static', tool_count: 0, tools: [], enabled: true, last_error: 'spawn ENOENT: design-system-mcp not found on PATH' },
];

const SKILLS: SkillEntry[] = [
  { name: 'rust-dev', description: 'Rust workflow, test targeting, clippy triage, and ergonomic code review for backend changes.', argument_hint: '[crate-or-test-filter]', source: 'builtin', path: '/Users/scottopell/.phoenix-ide/builtin-skills/rust-dev/SKILL.md' },
  { name: 'agent-browser', description: 'Drive a real browser for screenshots, repro steps, and exploratory UI testing with deterministic evidence.', argument_hint: '<url> [goal]', source: '/Users/scottopell/.agents/skills', path: '/Users/scottopell/.agents/skills/agent-browser/SKILL.md' },
  { name: 'phoenix-perf-hunt', description: 'Profiles Phoenix React scenarios with raw samples and coordinates one focused performance attempt.', source: `${ROOT}/.agents/skills`, path: `${ROOT}/.agents/skills/phoenix-perf-hunt/SKILL.md` },
  { name: 'very-long-project-specific-skill-name-for-truncation-review', description: 'Project skill with a deliberately long description that should wrap or truncate consistently without pushing counts and status metadata off screen.', argument_hint: '--scenario <name> --evidence <path>', source: `${ROOT}/.claude/skills`, path: `${ROOT}/.claude/skills/very-long-project-specific-skill-name-for-truncation-review/SKILL.md` },
];

const TASKS: TaskEntry[] = [
  { id: '22001', priority: 'p2', status: 'in-progress', slug: 'redesign-conversation-grounding-side-panel', path: `${ROOT}/tasks/22001-p2-in-progress--redesign-conversation-grounding-side-panel.md`, conversation_slug: 'grounding-panel-redesign' },
  { id: '22002', priority: 'p0', status: 'ready', slug: 'fix-critical-mcp-auth-refresh-loop-with-long-slug', path: `${ROOT}/tasks/22002-p0-ready--fix-critical-mcp-auth-refresh-loop-with-long-slug.md` },
  { id: '22003', priority: 'p1', status: 'blocked', slug: 'blocked-on-github-token-scope-decision', path: `${ROOT}/tasks/22003-p1-blocked--blocked-on-github-token-scope-decision.md`, conversation_slug: 'blocked-token-scope' },
  { id: '22004', priority: 'p3', status: 'brainstorming', slug: 'explore-grounding-panel-information-architecture', path: `${ROOT}/tasks/22004-p3-brainstorming--explore-grounding-panel-information-architecture.md` },
  { id: '22005', priority: 'p4', status: 'done', slug: 'archive-old-panel-screenshot-notes', path: `${ROOT}/tasks/22005-p4-done--archive-old-panel-screenshot-notes.md` },
  { id: '22006', priority: 'p2', status: 'wont-do', slug: 'replace-panel-with-floating-modal', path: `${ROOT}/tasks/22006-p2-wont-do--replace-panel-with-floating-modal.md` },
];

const WORK: WorkScopeInventory = {
  scope_key: SCOPE_KEY,
  bash: [
    { handle_id: 'b-1', label: 'vite dev server', cmd: 'pnpm --dir ui dev', state: 'running', pid: 42100, pgid: 42100, started_at: new Date(Date.now() - 75_000).toISOString(), output_bytes: 1_200_000 },
    { handle_id: 'b-2', label: 'stuck test cleanup', cmd: './dev.py check --lanes ui', state: 'kill_pending_kernel', pid: 42110, pgid: 42110, started_at: new Date(Date.now() - 185_000).toISOString(), output_bytes: 23_000 },
    { handle_id: 'b-3', label: 'seed fixture', cmd: './dev.py seed', state: 'tombstoned', started_at: new Date(Date.now() - 480_000).toISOString(), duration_ms: 34_000, exit_code: 0, output_bytes: 82_000 },
    { handle_id: 'b-4', label: 'failing lint', cmd: 'pnpm lint', state: 'tombstoned', started_at: new Date(Date.now() - 360_000).toISOString(), duration_ms: 19_000, exit_code: 2, output_bytes: 18_000 },
  ],
  tmux: { status: 'live' },
  browser: { state: 'live', idle_ms: 95_000 },
};

function json(data: unknown, init?: ResponseInit) {
  return new Response(JSON.stringify(data), { status: 200, headers: { 'content-type': 'application/json' }, ...init });
}

function installFixtureFetch(scenario: Scenario) {
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.pathname + input.search : input.url;
    if (url.startsWith('/api/files/read')) {
      const path = new URL(url, window.location.origin).searchParams.get('path') ?? '';
      const isTask = path.includes('/tasks/');
      return json({
        content: isTask
          ? '# Task brief\n\nRedesign the conversation grounding side panel with consistent sections, attention states, and reproducible QA screenshots.\n\n## Notes\n\n- Keep data scoped to the active conversation.\n- Make current task and blocked work visible.\n- Preserve keyboard access for rows and detail navigation.\n\nLong markdown paragraph: this task contains enough prose to exercise wrapping inside the detail view without making the panel wider than the minimum useful width.'
          : '# Skill: fixture detail\n\nUse this skill when a grounding panel screenshot needs long markdown content, argument hints, and enough prose to validate scrolling.\n\n## Arguments\n\n`--scenario <name> --evidence <path>`\n\n## Guidance\n\nPrefer deterministic fixtures over ad-hoc data. Capture screenshots after async sections settle and inspect console output for render errors.',
        kind: 'text',
        path,
      });
    }
    if (url.startsWith('/api/files/list')) {
      if (scenario === 'errors') return json({ error: 'fixture file list failed' }, { status: 500 });
      const path = new URL(url, window.location.origin).searchParams.get('path') ?? ROOT;
      return json({ items: scenario === 'empty' ? [] : FILES.get(path) ?? [] });
    }
    if (url === '/api/mcp/status') {
      if (scenario === 'errors') return json({ error: 'mcp unavailable' }, { status: 500 });
      return json(scenario === 'empty' ? [] : MCP);
    }
    if (url === `/api/conversations/${CONV_ID}/skills`) {
      if (scenario === 'errors') return json({ error: 'skills unavailable' }, { status: 500 });
      return json({ skills: scenario === 'empty' ? [] : SKILLS });
    }
    if (url === `/api/conversations/${CONV_ID}/tasks`) {
      if (scenario === 'errors') return json({ error: 'tasks unavailable' }, { status: 500 });
      return json({ tasks: scenario === 'empty' ? [] : TASKS });
    }
    if (url.startsWith('/api/work-scope/')) {
      if (scenario === 'errors') return json({ error: 'work scope unavailable' }, { status: 500 });
      return json(scenario === 'empty' ? { scope_key: SCOPE_KEY, bash: [], tmux: null, browser: null } : WORK);
    }
    return originalFetch(input, init);
  };
  return () => {
    window.fetch = originalFetch;
  };
}

export function GroundingPanelFixturePage() {
  const params = new URLSearchParams(window.location.search);
  const scenario = (params.get('scenario') as Scenario | null) ?? 'full';
  const theme = params.get('theme') ?? 'dark';
  const width = scenario === 'narrow' ? 248 : 360;
  const collapsed = scenario === 'collapsed';
  const [ready, setReady] = useState(false);

  useEffect(() => {
    document.documentElement.dataset['theme'] = theme;
    const restore = installFixtureFetch(scenario);
    setReady(true);
    return restore;
  }, [scenario, theme]);

  const liveWorkScope = useMemo(() => scenario === 'empty' || scenario === 'errors'
    ? { scope_key: SCOPE_KEY, bash: [], tmux: null, browser: null }
    : WORK, [scenario]);

  useEffect(() => {
    if (!ready) return;
    const timer = window.setTimeout(() => {
      const headers = [...document.querySelectorAll<HTMLButtonElement>('.grounding-section-header')];
      if (scenario === 'full' || scenario === 'empty' || scenario === 'errors') {
        for (const label of ['MCP', 'Skills', 'Tasks']) {
          headers.find((el) => el.textContent?.includes(label))?.click();
        }
        return;
      }
      if (scenario === 'skill-detail') {
        headers.find((el) => el.textContent?.includes('Skills'))?.click();
        window.setTimeout(() => document.querySelector<HTMLElement>('.skill-item')?.click(), 100);
      } else if (scenario === 'task-detail') {
        headers.find((el) => el.textContent?.includes('Tasks'))?.click();
        window.setTimeout(() => document.querySelector<HTMLElement>('.tasks-item')?.click(), 100);
      }
    }, 250);
    return () => window.clearTimeout(timer);
  }, [scenario, ready]);

  if (!ready) return null;

  return (
    <ViewerSlotProvider scopeKey="grounding-fixture" browserSessionActive={false}>
        <FileExplorerProvider>
        <main className="fixture-page">
          <div className="fixture-toolbar">
            <strong>Grounding panel fixture</strong>
            <span>scenario={scenario}</span>
            <span>theme={theme}</span>
          </div>
          <div className="fixture-panel-stage">
            <FileExplorerPanel
              collapsed={collapsed}
              onToggle={() => {}}
              rootPath={ROOT}
              conversationId={CONV_ID}
              showToast={() => {}}
              showError={() => {}}
              branchName="task-22001-redesign-conversation-grounding-side-panel"
              activeSlug="grounding-panel-redesign"
              width={width}
              workScopeKey={SCOPE_KEY}
              liveWorkScope={liveWorkScope}
            />
          </div>
        </main>
        </FileExplorerProvider>
    </ViewerSlotProvider>
  );
}
