import type { McpServerStatus, SkillEntry, TaskEntry } from '../api';

const TERMINAL_STATUSES = new Set(['done', 'wont-do']);
const BUILTIN_GROUP = 'Built-in';

function groupLabel(skill: SkillEntry): string {
  if (skill.source === 'builtin') return BUILTIN_GROUP;
  const markers = ['.claude/skills', '.agents/skills'];
  for (const marker of markers) {
    const idx = skill.path.indexOf(marker);
    if (idx !== -1) {
      const prefix = skill.path.substring(0, idx).replace(/\/$/, '');
      const lastSlash = prefix.lastIndexOf('/');
      const dirName = lastSlash >= 0 ? prefix.substring(lastSlash + 1) : prefix;
      if (!dirName || dirName === '~' || dirName === '') return 'User';
      if (/^\/Users\/[^/]+$/.test(prefix) || /^\/home\/[^/]+$/.test(prefix) || prefix === '~') {
        return 'User';
      }
      return dirName;
    }
  }
  return 'Other';
}

export function summarizeMcpStatus(servers: McpServerStatus[]): {
  ready: number;
  enabledReady: number;
  disabled: number;
  failed: number;
  pendingOAuth: number;
  tools: number;
  attention: boolean;
  label: string;
} {
  const readyServers = servers.filter(s => s.state === 'ready');
  const enabledReady = readyServers.filter(s => s.enabled);
  const disabled = readyServers.filter(s => !s.enabled).length;
  const failed = servers.filter(s => s.state === 'failed').length;
  const pendingOAuth = servers.filter(s => s.state === 'unauthorized').length;
  const tools = enabledReady.reduce((sum, s) => sum + s.tool_count, 0);
  const parts: string[] = [];
  if (enabledReady.length > 0) parts.push(`${enabledReady.length} ready`, `${tools} tools`);
  if (disabled > 0) parts.push(`${disabled} off`);
  if (pendingOAuth > 0) parts.push(`${pendingOAuth} auth`);
  if (failed > 0) parts.push(`${failed} failed`);
  return {
    ready: readyServers.length,
    enabledReady: enabledReady.length,
    disabled,
    failed,
    pendingOAuth,
    tools,
    attention: failed > 0 || pendingOAuth > 0,
    label: parts.join(' · ') || 'none configured',
  };
}

export function summarizeTasks(tasks: TaskEntry[], currentTaskId?: string): {
  active: number;
  closed: number;
  blocked: number;
  current: boolean;
  label: string;
} {
  const active = tasks.filter((t) => !TERMINAL_STATUSES.has(t.status)).length;
  const closed = tasks.length - active;
  const blocked = tasks.filter((t) => t.status === 'blocked').length;
  const current = currentTaskId != null && tasks.some((t) => t.id === currentTaskId);
  const parts = [`${active} active`];
  if (current) parts.push('current set');
  if (blocked > 0) parts.push(`${blocked} blocked`);
  if (closed > 0) parts.push(`${closed} closed`);
  return { active, closed, blocked, current, label: tasks.length === 0 ? 'not loaded' : parts.join(' · ') };
}

export function summarizeSkills(skills: SkillEntry[]): string {
  if (skills.length === 0) return 'none discovered';
  const groups = new Set(skills.map(groupLabel));
  return `${skills.length} available · ${groups.size} group${groups.size === 1 ? '' : 's'}`;
}
