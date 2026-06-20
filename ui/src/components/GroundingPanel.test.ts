import { describe, expect, it } from 'vitest';
import type { McpServerStatus, TaskEntry, SkillEntry } from '../api';
import { summarizeMcpStatus, summarizeSkills, summarizeTasks } from './groundingSummaries';

const tasks: TaskEntry[] = [
  { id: '1', priority: 'p0', status: 'in-progress', slug: 'current', path: '/tasks/1-p0-in-progress--current.md' },
  { id: '2', priority: 'p1', status: 'blocked', slug: 'blocked', path: '/tasks/2-p1-blocked--blocked.md' },
  { id: '3', priority: 'p4', status: 'done', slug: 'closed', path: '/tasks/3-p4-done--closed.md' },
];

const skills: SkillEntry[] = [
  { name: 'builtin', description: 'built in', source: 'builtin', path: '/builtin/SKILL.md' },
  { name: 'project', description: 'project', source: '/repo/.agents/skills', path: '/repo/.agents/skills/project/SKILL.md' },
];

describe('grounding panel summaries', () => {
  it('surfaces MCP attention for auth and failed servers', () => {
    const servers: McpServerStatus[] = [
      { name: 'ok', state: 'ready', transport: 'stdio', auth: 'none', tool_count: 3, tools: ['a', 'b', 'c'], enabled: true },
      { name: 'off', state: 'ready', transport: 'stdio', auth: 'none', tool_count: 2, tools: ['x', 'y'], enabled: false },
      { name: 'auth', state: 'unauthorized', transport: 'http', auth: 'oauth', tool_count: 0, tools: [], enabled: true },
      { name: 'bad', state: 'failed', transport: 'stdio', auth: 'static', tool_count: 0, tools: [], enabled: true },
    ];

    const summary = summarizeMcpStatus(servers);

    expect(summary.enabledReady).toBe(1);
    expect(summary.disabled).toBe(1);
    expect(summary.pendingOAuth).toBe(1);
    expect(summary.failed).toBe(1);
    expect(summary.attention).toBe(true);
    expect(summary.label).toContain('1 auth');
    expect(summary.label).toContain('1 failed');
  });

  it('summarizes task current, blocked, active, and closed counts', () => {
    const summary = summarizeTasks(tasks, '1');

    expect(summary.active).toBe(2);
    expect(summary.closed).toBe(1);
    expect(summary.blocked).toBe(1);
    expect(summary.current).toBe(true);
    expect(summary.label).toBe('2 active · current set · 1 blocked · 1 closed');
  });

  it('summarizes skills by discovered groups', () => {
    expect(summarizeSkills(skills)).toBe('2 available · 2 groups');
    expect(summarizeSkills([])).toBe('none discovered');
  });
});
