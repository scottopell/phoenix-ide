import * as v from 'valibot';
import type { ApplicationCapabilities } from './generated/ApplicationCapabilities';
import type { PersistedMemberResponse } from './generated/PersistedMemberResponse';
import { InputOriginSchema } from './sseSchemas';

const availability = v.picklist(['available', 'unavailable']);
export const ApplicationCapabilitiesSchema = v.object({
  persisted_member_read: availability, live_stream: availability,
  conversation_activation: availability, user_input: availability, telemetry: availability,
  provider_catalog: availability, host_resource_observations: availability,
}) satisfies v.GenericSchema<unknown, ApplicationCapabilities>;

const nullableString = v.nullable(v.string());
const integer = v.pipe(v.number(), v.integer(), v.minValue(0));
export const PersistedMemberResponseSchema = v.object({
  source: v.literal('persisted'),
  conversation: v.object({
    id: v.string(), slug: nullableString, title: nullableString, cwd: v.string(),
    parent_conversation_id: nullableString, user_initiated: v.boolean(), state: v.unknown(),
    state_updated_at: v.string(), created_at: v.string(), updated_at: v.string(), archived: v.boolean(),
    model: nullableString, effort: v.nullable(v.picklist(['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'])),
    service_tier: v.picklist(['standard', 'fast']), project_id: nullableString,
    message_count: integer, transcript_generation: integer, seed_parent_id: nullableString,
    seed_label: nullableString, continued_in_conv_id: nullableString, chain_name: nullableString,
    conv_mode_label: v.string(), branch_name: nullableString, worktree_path: nullableString,
    base_branch: nullableString, task_title: nullableString, work_scope_key: v.string(),
    seed_parent_slug: nullableString, parent_conversation_slug: nullableString,
    cached_pr: v.nullable(v.object({
      number: integer, title: v.string(), url: v.string(),
      display_state: v.picklist(['open', 'draft', 'closed', 'merged']), base: v.string(), head: v.string(),
      feedback_status: v.picklist(['open', 'in_progress', 'approved']),
    })),
  }),
  messages: v.array(v.object({
    message_id: v.string(), conversation_id: v.string(), sequence_id: integer,
    message_type: v.picklist(['user', 'agent', 'tool', 'system', 'error', 'continuation', 'skill']), content: v.unknown(), origin: InputOriginSchema,
    display_data: v.unknown(), usage_data: v.nullable(v.object({
      input_tokens: integer, output_tokens: integer, reasoning_tokens: v.nullable(integer),
      cache_creation_tokens: integer, cache_read_tokens: integer,
    })), created_at: v.string(),
  })),
  transcript_coverage: v.picklist(['complete', 'tail']), server_message_tail: v.nullable(integer),
  persisted_agent_working: v.boolean(), live_process: v.object({ availability: v.literal('unavailable') }),
}) satisfies v.GenericSchema<unknown, PersistedMemberResponse>;
