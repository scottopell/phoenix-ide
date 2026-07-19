import { describe, expect, it } from 'vitest';
import type { Conversation, Project } from '../api';
import {
  getConversationDisplayTitle,
  getConversationIdentityDisplay,
  getConversationProjectLabel,
  getProjectDisplayLabel,
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
    }))).toBeNull();

    expect(getConversationProjectLabel(makeConversation({
      project_name: null,
      cwd: '/tmp/fork-9d1b4cc93b7845228e4fdbe566761f44',
    }))).toBeNull();
  });

  it('derives a readable project label from canonical project paths only', () => {
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/Users/scott/phoenix-ide' }))).toBe('phoenix-ide');
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/repo/.phoenix/worktrees/task-123' }))).toBeNull();
    expect(getProjectDisplayLabel(makeProject({ canonical_path: '/tmp/9d1b4cc93b7845228e4fdbe566761f44' }))).toBeNull();
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
});
