import { describe, expect, it } from 'vitest';
import type { Message, ToolResultContent } from '../../api';
import { getToolResultsScenario, toolResultsFixtureData, toolResultsScenarios } from './scenarios';
import type { ToolResultsScenarioFamily } from './types';
import type { ToolResultsScenarioId } from './scenarios';

function familyData(family: ToolResultsScenarioFamily) {
  return toolResultsFixtureData(getToolResultsScenario(`${family}-full` as ToolResultsScenarioId));
}

function toolUses(messages: Message[]) {
  return messages.flatMap((message) => {
    if (message.message_type !== 'agent' || !Array.isArray(message.content)) return [];
    return message.content.flatMap((block) => block.type === 'tool_use' && block.id
      ? [{ id: block.id, name: block.name || 'tool', input: (block.input || {}) as Record<string, unknown> }]
      : []);
  });
}

function toolResults(messages: Message[]) {
  return messages.flatMap((message) => message.message_type === 'tool'
    ? [{ message, content: message.content as ToolResultContent }]
    : []);
}

function unpairedIds(messages: Message[]) {
  const paired = new Set(toolResults(messages).map(({ content }) => content.tool_use_id));
  return toolUses(messages).map(({ id }) => id).filter((id) => !paired.has(id)).sort();
}

function resultFor(messages: Message[], id: string) {
  return toolResults(messages).find(({ content }) => content.tool_use_id === id);
}

const expectedFamilies: ToolResultsScenarioFamily[] = ['shell', 'execution', 'discovery', 'media', 'profiling', 'subagents'];

const expectedIds = expectedFamilies.flatMap((family) => [`${family}-full`, `${family}-compact`]);

function expectTextStates(messages: Message[], states: Record<string, string>) {
  for (const [id, marker] of Object.entries(states)) {
    expect(resultFor(messages, id)?.content.content).toContain(marker);
  }
}

describe('tool results fixture scenarios', () => {
  it('has unique full/compact stories backed by the same deterministic family data', () => {
    expect(toolResultsScenarios.map(({ id }) => id)).toEqual(expectedIds);
    expect(new Set(expectedIds).size).toBe(expectedIds.length);

    for (const family of expectedFamilies) {
      const full = toolResultsFixtureData(getToolResultsScenario(`${family}-full` as ToolResultsScenarioId));
      const compact = toolResultsFixtureData(getToolResultsScenario(`${family}-compact` as ToolResultsScenarioId));
      expect(full.messages).toBe(compact.messages);
      expect(full.density).toBe('full');
      expect(compact.density).toBe('compact');
    }

    for (const scenario of toolResultsScenarios) {
      const data = toolResultsFixtureData(scenario);
      expect(data.slug).toContain(scenario.id);
      expect(data.conversationId).toContain(scenario.id);
      expect(data.theme).toBe('dark');
      expect(data.density).toBe(scenario.density);
      expect(data.pendingMessages).toEqual([]);
      expect(data.messages.length).toBeGreaterThanOrEqual(4);
      expect(data.messages.every((message) => message.conversation_id === 'fixture-tool-results')).toBe(true);
    }
  });

  it('pairs every tool result exactly once except intentional active/missing cases', () => {
    const expectedUnpaired: Record<ToolResultsScenarioFamily, string[]> = {
      shell: ['shell-missing', 'shell-pending', 'shell-think'],
      execution: [],
      discovery: [],
      media: [],
      profiling: ['profile-run-missing'],
      subagents: [],
    };

    for (const family of expectedFamilies) {
      const messages = familyData(family).messages;
      const resultIds = toolResults(messages).map(({ content }) => content.tool_use_id);
      expect(new Set(resultIds).size).toBe(resultIds.length);
      expect(unpairedIds(messages)).toEqual(expectedUnpaired[family]);
    }
  });

  it('covers lifecycle, specialized browser inputs, and explicit unknown JSON fallback', () => {
    const data = familyData('shell');
    const uses = toolUses(data.messages);
    expect(data.convState).toMatchObject({ type: 'tool_executing', current_tool: { id: 'shell-pending' } });
    const pendingMessage = data.messages.find((message) => message.message_id === 'agent-13');
    expect(pendingMessage?.display_data).toMatchObject({ tool_starts: { 'shell-pending': 0 } });
    expect(uses.map(({ name }) => name)).toEqual([
      'think',
      'browser_click',
      'browser_navigate',
      'browser_clear_console_logs',
      'browser_eval',
      'browser_type',
      'browser_key_press',
      'browser_resize',
      'ask_user_question',
      'read_file',
      'propose_task',
      'custom_fixture_tool',
      'browser_wait_for_selector',
    ]);
    expect(resultFor(data.messages, 'shell-empty')?.content.content).toBe('');
    expect(resultFor(data.messages, 'shell-error')?.content.is_error).toBe(true);
    expect(resultFor(data.messages, 'shell-long')?.content.content?.split('\n')).toHaveLength(20);
    expect(resultFor(data.messages, 'shell-truncated')?.content.content?.length).toBeGreaterThan(5_000);
    expect(resultFor(data.messages, 'shell-truncated')?.content.content).not.toContain('more chars)');
    expect(resultFor(data.messages, 'shell-proposal')?.message.display_data).toMatchObject({ fork_proposal_id: 'fixture-fork-proposal' });
    expect(resultFor(data.messages, 'shell-unknown')?.content.content).toContain('unknown tool renderer');
  });

  it('covers skill, bash, tmux, and patch structured/error/legacy states', () => {
    const messages = familyData('execution').messages;
    expect(new Set(toolUses(messages).map(({ name }) => name))).toEqual(new Set(['skill', 'bash', 'tmux', 'patch']));

    expectTextStates(messages, {
      'exec-bash-running': '"status":"running"',
      'exec-bash-still': '"status":"still_running"',
      'exec-bash-kill-pending': '"status":"kill_pending_kernel"',
      'exec-bash-exited': '"status":"exited"',
      'exec-bash-killed': '"status":"killed"',
      'exec-bash-tombstoned': '"status":"tombstoned"',
      'exec-bash-error': '"error":"handle_not_found"',
      'exec-bash-legacy': ' M ui/src/components/MessageComponents.tsx',
      'exec-tmux-ok': '"stdout"',
      'exec-tmux-stderr': '"stderr"',
      'exec-tmux-truncated': '"truncated":true',
      'exec-tmux-error': '"error":"tmux_wait_failed"',
      'exec-tmux-plain': 'legacy tmux capture output',
      'exec-patch-legacy': '--- a/README.md',
    });
    expect(resultFor(messages, 'exec-patch-trivial')?.message.display_data).toHaveProperty('diff');
    expect(resultFor(messages, 'exec-patch-multi')?.message.display_data).toHaveProperty('diff');
    expect(resultFor(messages, 'exec-patch-error')?.content.is_error).toBe(true);
    expect(resultFor(messages, 'exec-skill-error')?.content.is_error).toBe(true);
  });

  it('covers structured, empty, and raw fallback search result shapes', () => {
    const messages = familyData('discovery').messages;
    expect(toolUses(messages).map(({ name }) => name)).toEqual([
      'search', 'search', 'search', 'keyword_search', 'keyword_search', 'keyword_search',
      'read_file', 'read_file', 'read_file', 'read_file', 'read_file', 'read_file', 'read_file', 'read_file',
    ]);
    expectTextStates(messages, {
      'discover-search-empty': 'No matches found.',
      'discover-search-raw': 'permission denied',
      'discover-keyword-empty': 'No relevant files found',
      'discover-keyword-raw': '--',
    });
    expectTextStates(messages, {
      'discover-search-empty': 'No matches found.',
      'discover-search-raw': 'permission denied',
      'discover-keyword-empty': 'No relevant files found',
      'discover-keyword-raw': '--',
      'discover-read-short': 'deterministicFixtureLine1',
      'discover-read-long': 'fixture long line 24',
      'discover-read-range': 'MessageComponents excerpt 711',
      'discover-read-eof': 'EOF excerpt 501',
      'discover-read-long-lines': 'wrap me please',
      'discover-read-malformed': 'valid line',
    });
    expect(resultFor(messages, 'discover-read-empty')?.content.content).toBe('');
    expect(resultFor(messages, 'discover-read-error')?.content.is_error).toBe(true);
  });

  it('covers image compatibility and console structured/empty/pointer/raw paths', () => {
    const messages = familyData('media').messages;
    expect(resultFor(messages, 'media-read-image-typed')?.content.images).toHaveLength(1);
    expect(resultFor(messages, 'media-screenshot-display')?.message.display_data).toMatchObject({ type: 'image', media_type: 'image/svg+xml' });
    expect(resultFor(messages, 'media-read-image-legacy')?.content.content).toContain('"type":"image"');
    expect(resultFor(messages, 'media-read-image-malformed')?.content.content).toBe('{"type":"image","media_type":"image/png"}');
    expect(resultFor(messages, 'media-console-empty')?.content.content).toBe('[]');
    expect(resultFor(messages, 'media-console-structured')?.content.content).toContain('synthetic renderer warning');
    expect(resultFor(messages, 'media-console-pointer')?.content.content).toContain('Logs written to /tmp/phoenix-console-fixture.log');
    expect(resultFor(messages, 'media-console-unparseable')?.content.content).toContain('{bad json console payload');
  });

  it('covers every structured browser_profile action plus blocked/error/missing/generic paths', () => {
    const messages = familyData('profiling').messages;
    const actions = toolUses(messages)
      .filter(({ name }) => name === 'browser_profile')
      .map(({ input }) => input['action']);
    expect(actions).toEqual([
      'run_scenario', 'run_scenario', 'run_scenario', 'run_scenario',
      'metrics', 'cpu_stop', 'cpu_summary', 'trace_stop', 'heap_snapshot', 'why_render',
    ]);
    expect(resultFor(messages, 'profile-run-completed')?.message.display_data).toMatchObject({ outcome: 'completed' });
    expect(resultFor(messages, 'profile-run-blocked')?.message.display_data).toMatchObject({ outcome: 'blocked' });
    const profileError = resultFor(messages, 'profile-run-error');
    expect(profileError?.content.is_error).toBe(true);
    expect(profileError?.message).not.toHaveProperty('display_data');
    expect(resultFor(messages, 'profile-generic')?.content.content).toContain('browser profile raw text fallback');
  });

  it('uses tasks-shaped spawn_agents input and persists success/failure/timeout outcomes', () => {
    const messages = familyData('subagents').messages;
    const spawn = toolUses(messages).find(({ name }) => name === 'spawn_agents');
    expect(spawn?.input['tasks']).toBeInstanceOf(Array);
    expect(spawn?.input).not.toHaveProperty('agents');
    const displayData = resultFor(messages, 'subagents-spawn')?.message.display_data as { type?: string; results?: Array<{ outcome: { type: string } }> };
    expect(displayData.type).toBe('subagent_summary');
    expect(displayData.results?.map(({ outcome }) => outcome.type).sort()).toEqual(['failure', 'success', 'timed_out']);
  });
});
