import type { GroundingPanelFixtureData, GroundingPanelScenario } from './types';
import { emptyWorkScope, groundingPanelFixtureData } from './scenarios';

function json(data: unknown, init?: ResponseInit) {
  return new Response(JSON.stringify(data), { status: 200, headers: { 'content-type': 'application/json' }, ...init });
}

export function fixtureWorkScope(scenario: GroundingPanelScenario) {
  return scenario.kind === 'empty' || scenario.kind === 'errors' ? emptyWorkScope() : groundingPanelFixtureData.workScope;
}

export function installGroundingPanelFixtureFetch(
  scenario: GroundingPanelScenario,
  data: GroundingPanelFixtureData = groundingPanelFixtureData,
) {
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.pathname + input.search : input.url;
    if (url.startsWith('/api/files/read')) {
      const path = new URL(url, window.location.origin).searchParams.get('path') ?? '';
      const isTask = path.includes('/tasks/');
      return json({ content: isTask ? data.taskDetailMarkdown : data.skillDetailMarkdown, kind: 'text', path });
    }
    if (url.startsWith('/api/files/list')) {
      if (scenario.kind === 'errors') return json({ error: 'fixture file list failed' }, { status: 500 });
      const path = new URL(url, window.location.origin).searchParams.get('path') ?? scenario.rootPath;
      return json({ items: scenario.kind === 'empty' ? [] : data.files.get(path) ?? [] });
    }
    if (url === '/api/mcp/status') {
      if (scenario.kind === 'errors') return json({ error: 'mcp unavailable' }, { status: 500 });
      return json(scenario.kind === 'empty' ? [] : data.mcp);
    }
    if (url === `/api/conversations/${scenario.conversationId}/skills`) {
      if (scenario.kind === 'errors') return json({ error: 'skills unavailable' }, { status: 500 });
      return json({ skills: scenario.kind === 'empty' ? [] : data.skills });
    }
    if (url.startsWith(`/api/conversations/${scenario.conversationId}/tasks/count`)) {
      if (scenario.kind === 'errors') return json({ error: 'task counts unavailable' }, { status: 500 });
      const tasks = scenario.kind === 'empty' ? [] : data.tasks;
      const currentId = new URL(url, window.location.origin).searchParams.get('current_task_id');
      const terminal = new Set(['done', 'wont-do']);
      const active = tasks.filter((t) => !terminal.has(t.status)).length;
      return json({
        active,
        closed: tasks.length - active,
        blocked: tasks.filter((t) => t.status === 'blocked').length,
        current: currentId != null && tasks.some((t) => t.id === currentId),
      });
    }
    if (url === `/api/conversations/${scenario.conversationId}/tasks`) {
      if (scenario.kind === 'errors') return json({ error: 'tasks unavailable' }, { status: 500 });
      return json({ tasks: scenario.kind === 'empty' ? [] : data.tasks });
    }
    if (url.startsWith('/api/work-scope/')) {
      if (scenario.kind === 'errors') return json({ error: 'work scope unavailable' }, { status: 500 });
      return json(scenario.kind === 'empty' ? emptyWorkScope() : data.workScope);
    }
    return originalFetch(input, init);
  };
  return () => {
    window.fetch = originalFetch;
  };
}
