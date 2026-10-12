import type { ApplicationCapabilities } from '../generated/ApplicationCapabilities';
import type { PersistedMemberResponse } from '../generated/PersistedMemberResponse';

export const persistedCapabilities: ApplicationCapabilities = {
  persisted_member_read: 'available', live_stream: 'unavailable', conversation_activation: 'unavailable',
  user_input: 'unavailable', telemetry: 'unavailable', provider_catalog: 'unavailable', host_resource_observations: 'unavailable',
};

export function persistedObservationFixture(): PersistedMemberResponse {
  return {
    source: 'persisted', transcript_coverage: 'tail', server_message_tail: 2,
    persisted_agent_working: true, live_process: { availability: 'unavailable' },
    conversation: {
      id: 'persisted-member', slug: 'saved-member', title: 'Saved conversation', cwd: '/server/project',
      parent_conversation_id: null, user_initiated: true, state: { type: 'awaiting_sub_agents', pending: [], completed_results: [] },
      state_updated_at: '2026-10-01T10:00:00Z', created_at: '2026-10-01T09:00:00Z', updated_at: '2026-10-01T10:00:00Z',
      archived: false, model: 'recorded-model', effort: null, service_tier: 'standard', project_id: null,
      message_count: 12, transcript_generation: 7, seed_parent_id: null, seed_label: null, continued_in_conv_id: null,
      chain_name: null, conv_mode_label: 'work', branch_name: 'recorded-branch', worktree_path: null, base_branch: 'main',
      task_title: null, work_scope_key: 'conversation:persisted-member', seed_parent_slug: null,
      parent_conversation_slug: null, cached_pr: null,
    },
    messages: [{
      message_id: 'saved-message', conversation_id: 'persisted-member', sequence_id: 12, message_type: 'agent',
      content: { text: 'Saved transcript content', blocks: [{ type: 'text', text: 'Saved transcript content' }] },
      origin: { kind: 'user_api' }, display_data: null, usage_data: null, created_at: '2026-10-01T10:00:00Z',
    }],
  };
}
