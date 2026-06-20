import type { McpServerStatus, SkillEntry, TaskEntry } from '../api';

const TERMINAL_STATUSES = new Set(['done', 'wont-do']);
const BUILTIN_GROUP = 'Built-in';

/**
 * Group label for a skill. Built-in skills are pulled into a dedicated
 * "Built-in" group; filesystem skills derive their label from the directory
 * above `.claude/skills` / `.agents/skills`. Single source for both the
 * collapsed-header group count (`summarizeSkills`) and the expanded body's
 * group headers (`SkillsPanel`), so the two never disagree.
 */
export function groupLabel(skill: SkillEntry): string {
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

/**
 * Group skills by {@link groupLabel}. The "Built-in" group is pulled to the
 * front of the resulting Map (Maps preserve insertion order) so phoenix-bundled
 * skills render above filesystem ones.
 */
export function groupSkills(skills: SkillEntry[]): Map<string, SkillEntry[]> {
  const groups = new Map<string, SkillEntry[]>();
  for (const skill of skills) {
    const label = groupLabel(skill);
    const existing = groups.get(label);
    if (existing) {
      existing.push(skill);
    } else {
      groups.set(label, [skill]);
    }
  }
  if (groups.has(BUILTIN_GROUP)) {
    const builtins = groups.get(BUILTIN_GROUP)!;
    groups.delete(BUILTIN_GROUP);
    const reordered = new Map<string, SkillEntry[]>();
    reordered.set(BUILTIN_GROUP, builtins);
    for (const [k, v] of groups) reordered.set(k, v);
    return reordered;
  }
  return groups;
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

/** Status counts behind the Tasks header. Produced either by reducing the full
 *  task list (expanded) or by the lightweight count endpoint (collapsed); both
 *  render through {@link taskCountsLabel} so the header reads identically. */
export interface TaskCounts {
  active: number;
  closed: number;
  blocked: number;
  current: boolean;
}

/** Header summary string for a set of task counts. `loaded` is false before any
 *  counts have arrived, so the header shows a placeholder rather than "0 active". */
export function taskCountsLabel(counts: TaskCounts, loaded: boolean): string {
  if (!loaded) return 'not loaded';
  const parts = [`${counts.active} active`];
  if (counts.current) parts.push('current set');
  if (counts.blocked > 0) parts.push(`${counts.blocked} blocked`);
  if (counts.closed > 0) parts.push(`${counts.closed} closed`);
  return parts.join(' · ');
}

export function summarizeTasks(
  tasks: TaskEntry[],
  currentTaskId?: string,
): TaskCounts & { label: string } {
  const active = tasks.filter((t) => !TERMINAL_STATUSES.has(t.status)).length;
  const closed = tasks.length - active;
  const blocked = tasks.filter((t) => t.status === 'blocked').length;
  const current = currentTaskId != null && tasks.some((t) => t.id === currentTaskId);
  const counts = { active, closed, blocked, current };
  return { ...counts, label: taskCountsLabel(counts, tasks.length > 0) };
}

export function summarizeSkills(skills: SkillEntry[]): string {
  if (skills.length === 0) return 'none discovered';
  const groups = new Set(skills.map(groupLabel));
  return `${skills.length} available · ${groups.size} group${groups.size === 1 ? '' : 's'}`;
}
