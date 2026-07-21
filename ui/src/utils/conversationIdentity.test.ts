import { describe, expect, it } from 'vitest';
import type { Conversation, Project } from '../api';
import {
  getConversationDisplayTitle,
  getConversationIdentity,
  getConversationIdentityDisplay,
  getConversationProjectLabel,
  getProjectDisplayLabel,
  getPathDisplayLabel,
  getDisambiguatedPathLabels,
  summarizeConversationPath,
} from './conversationIdentity';

function makeConversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: 'conv-1',
    slug: 'readable-slug',
    cwd: '/repo/readable-project',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
    message_count: 1,
    model: 'claude-3-5-sonnet',
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'conversation:conv-1',
    ...overrides,
  };
}

function makeProject(overrides: Partial<Project> = {}): Project {
  return {
    id: 'proj-1',
    canonical_path: '/repo/readable-project',
    main_ref: 'main',
    created_at: '2024-01-01T00:00:00Z',
    conversation_count: 1,
    ...overrides,
  };
}

describe('conversationIdentity', () => {
  it('uses backend project_name when it is human-readable', () => {
    expect(getConversationProjectLabel(makeConversation({
      project_name: 'Phoenix IDE',
      cwd: '/tmp/ignored',
    }))).toBe('Phoenix IDE');
  });

  it('rejects worktree and hash-like project leaves for conversation labels', () => {
    expect(getConversationProjectLabel(makeConversation({
      project_name: null,
      cwd: '/repo/.phoenix/worktrees/123e4567-e89b-12d3-a456-426614174000',
    }))).toBe('repo');

    expect(getConversationProjectLabel(makeConversation({
      project_name: null,
      cwd: '/tmp/fork-9d1b4cc93b7845228e4fdbe566761f44',
    }))).toBeNull();
  });

  it('derives a readable project label from canonical project paths only', () => {
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/Users/scott/phoenix-ide' }))).toBe('phoenix-ide');
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/repo/.phoenix/worktrees/task-123' }))).toBe('repo');
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/tmp/9d1b4cc93b7845228e4fdbe566761f44' }))).toBeNull();
  });

  it('derives semantic labels for managed and seeded worktree paths', () => {
    expect(getPathDisplayLabel('/Users/scott/phoenix-ide/.phoenix/worktrees/123e4567-e89b-12d3-a456-426614174000')).toBe('phoenix-ide');
    expect(getPathDisplayLabel('/Users/scott/phoenix-ide/.phoenix/seed-worktrees/grounding-panel-qa')).toBe('grounding-panel-qa');
  });

  it('disambiguates colliding project labels with meaningful parents and stable ordinals', () => {
    const labels = getDisambiguatedPathLabels([
      '/Users/scott/client-a/app',
      '/Users/scott/client-b/app',
      '/Users/scott/repo/.phoenix/worktrees/11111111-1111-4111-8111-111111111111',
      '/Users/scott/repo/.phoenix/worktrees/22222222-2222-4222-8222-222222222222',
    ]);
    expect(labels.get('/Users/scott/client-a/app')).toBe('client-a/app');
    expect(labels.get('/Users/scott/client-b/app')).toBe('client-b/app');
    expect(labels.get('/Users/scott/repo/.phoenix/worktrees/11111111-1111-4111-8111-111111111111')).toBe('scott/repo · 1');
    expect(labels.get('/Users/scott/repo/.phoenix/worktrees/22222222-2222-4222-8222-222222222222')).toBe('scott/repo · 2');
  });

  it('keeps meaningful titles that contain UUIDs or long hashes', () => {
    expect(getConversationDisplayTitle(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Investigate 123e4567-e89b-12d3-a456-426614174000 failure',
    }))).toBe('Investigate 123e4567-e89b-12d3-a456-426614174000 failure');
    expect(getConversationDisplayTitle(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Review commit 9d1b4cc93b7845228e4fdbe566761f44',
    }))).toBe('Review commit 9d1b4cc93b7845228e4fdbe566761f44');
  });

  it('falls back to semantic title sources when slug is guid-like', () => {
    expect(getConversationDisplayTitle(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Readable task title',
    }))).toBe('Readable task title');

    expect(getConversationDisplayTitle(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: null,
      branch_name: 'scott/readable-branch',
    }))).toBe('scott/readable-branch');
  });

  it('rejects generated prefixed UUID slugs but honors renamed slugs before task titles', () => {
    expect(getConversationDisplayTitle(makeConversation({
      slug: 'fork-123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Readable task title',
    }))).toBe('Readable task title');

    expect(getConversationDisplayTitle(makeConversation({
      slug: 'renamed-work-conversation',
      task_title: 'Original task title',
    }))).toBe('renamed-work-conversation');
  });

  it('rejects generated short-id slugs without rejecting ordinary hex-like words', () => {
    expect(getConversationDisplayTitle(makeConversation({
      slug: 'fork-c8f92b',
      task_title: 'Readable task title',
    }))).toBe('Readable task title');
    expect(getConversationDisplayTitle(makeConversation({
      slug: 'debug-deadbeef-caching',
      task_title: 'Original task title',
    }))).toBe('debug-deadbeef-caching');
  });

  it('rejects generated prefixed long hashes in explicit project names', () => {
    expect(getConversationProjectLabel(makeConversation({
      project_name: 'fork-9d1b4cc93b7845228e4fdbe566761f44',
      cwd: '/Users/scott/phoenix-ide',
    }))).toBe('phoenix-ide');
  });

  it('returns a shared typed identity display payload', () => {
    expect(getConversationIdentityDisplay(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Readable task title',
      project_name: 'Phoenix IDE',
    }))).toEqual({
      title: 'Readable task title',
      projectLabel: 'Phoenix IDE',
    });
  });

  it('builds a shared typed identity payload for desktop and mobile consumers', () => {
    expect(getConversationIdentity(makeConversation({
      slug: '123e4567-e89b-12d3-a456-426614174000',
      task_title: 'Readable task title',
      project_name: 'Phoenix IDE',
      branch_name: 'task-78001-redesign-state-bar',
      base_branch: 'main',
      conv_mode_label: 'wOrK',
      model: 'claude-sonnet-5',
      cwd: '/Users/scott/projects/phoenix-ide',
    }))).toEqual({
      title: 'Readable task title',
      projectLabel: 'Phoenix IDE',
      taskTitle: 'Readable task title',
      branch: { active: 'task-78001-redesign-state-bar', base: 'main' },
      path: { full: '/Users/scott/projects/phoenix-ide', summary: '…/projects/phoenix-ide' },
      mode: {
        key: 'work',
        label: 'Work',
        title: 'Work mode: task branch',
        detail: 'Task branch',
        desktopDetail: 'task branch',
      },
      modelLabel: 'claude-sonnet-5',
    });
  });

  it('prefers worktree_path over cwd for shared path identity', () => {
    expect(getConversationIdentity(makeConversation({
      slug: 'task-conversation',
      project_name: null,
      worktree_path: '/repo/.phoenix/worktrees/task-78001-redesign-state-bar',
      cwd: '/repo/.phoenix/worktrees/conv-1',
    }))).toMatchObject({
      path: {
        full: '/repo/.phoenix/worktrees/task-78001-redesign-state-bar',
        summary: '…/worktrees/task-78001-redesign-state-bar',
      },
    });
  });

  it('summarizes conversation paths for mobile cwd display', () => {
    expect(summarizeConversationPath('/Users/scott/projects/phoenix-ide')).toBe('…/projects/phoenix-ide');
    expect(summarizeConversationPath('/repo')).toBe('/repo');
    expect(summarizeConversationPath(null)).toBe('—');
  });
});
