ALTER TABLE turn_usage ADD COLUMN capacity_plan_id TEXT REFERENCES request_capacity_plans(plan_id) ON DELETE SET NULL;
ALTER TABLE llm_request_metrics ADD COLUMN capacity_plan_id TEXT REFERENCES request_capacity_plans(plan_id) ON DELETE SET NULL;
ALTER TABLE turn_usage ADD COLUMN request_id TEXT;
ALTER TABLE turn_usage ADD COLUMN retry_attempt INTEGER CHECK((request_id IS NULL AND retry_attempt IS NULL) OR (request_id IS NOT NULL AND retry_attempt IS NOT NULL AND retry_attempt >= 1));
ALTER TABLE turn_usage ADD COLUMN input_total_observed INTEGER CHECK(input_total_observed IS NULL OR input_total_observed IN (0,1));
ALTER TABLE turn_usage ADD COLUMN output_observed INTEGER CHECK(output_observed IS NULL OR output_observed IN (0,1));
ALTER TABLE turn_usage ADD COLUMN cache_write_observed INTEGER CHECK(cache_write_observed IS NULL OR cache_write_observed IN (0,1));
ALTER TABLE turn_usage ADD COLUMN cache_read_observed INTEGER CHECK(cache_read_observed IS NULL OR cache_read_observed IN (0,1))
 CHECK((request_id IS NULL AND input_total_observed IS NULL AND output_observed IS NULL AND cache_write_observed IS NULL AND cache_read_observed IS NULL)
 OR (request_id IS NOT NULL AND input_total_observed IS NOT NULL AND output_observed IS NOT NULL AND cache_write_observed IS NOT NULL AND cache_read_observed IS NOT NULL));
CREATE UNIQUE INDEX turn_usage_request_attempt ON turn_usage(request_id, retry_attempt) WHERE request_id IS NOT NULL;
CREATE TABLE request_capacity_plans (
 plan_id TEXT PRIMARY KEY NOT NULL CHECK(length(plan_id) > 0),
 request_id TEXT NOT NULL,
 retry_attempt INTEGER NOT NULL CHECK(retry_attempt >= 1),
 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
 root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
 kind TEXT NOT NULL CHECK(kind IN ('ordinary','compaction','handoff_shortening','successor_projection')),
 model TEXT NOT NULL,
 route_class TEXT NOT NULL CHECK(route_class IN ('unknown','anthropic_direct','anthropic_compatible','open_ai_responses_direct','codex_auth','open_ai_responses_compatible','chat_completions_direct','chat_completions_compatible')),
 connection_id TEXT,
 context_window_tokens INTEGER NOT NULL CHECK(context_window_tokens >= 0),
 output_reservation_tokens INTEGER NOT NULL CHECK(output_reservation_tokens >= 0),
 local_prediction_tokens INTEGER NOT NULL CHECK(local_prediction_tokens >= 0),
 provider_input_tokens INTEGER CHECK(provider_input_tokens IS NULL OR provider_input_tokens >= 0),
 superseded_local_prediction_tokens INTEGER CHECK(superseded_local_prediction_tokens IS NULL OR superseded_local_prediction_tokens >= 0),
 superseded_provider_input_tokens INTEGER CHECK(superseded_provider_input_tokens IS NULL OR superseded_provider_input_tokens >= 0),
 superseded_counting_method TEXT CHECK(superseded_counting_method IS NULL OR superseded_counting_method IN ('conservative_local_estimate','open_ai_complete_input','anthropic_estimate')),
 superseded_count_fallback TEXT CHECK(superseded_count_fallback IS NULL OR superseded_count_fallback IN ('local_ready','budget_spent','unavailable','failed','timed_out')),
 counting_method TEXT NOT NULL CHECK(counting_method IN ('conservative_local_estimate','open_ai_complete_input','anthropic_estimate')),
 count_fallback TEXT CHECK(count_fallback IS NULL OR count_fallback IN ('local_ready','budget_spent','unavailable','failed','timed_out')),
 reduction_action TEXT NOT NULL CHECK(reduction_action IN ('none','clearing','bounded_history')),
 reduced_local_tokens INTEGER NOT NULL CHECK(reduced_local_tokens >= 0),
 outcome TEXT NOT NULL CHECK(outcome IN ('ready','needs_compaction','capacity_conflict')),
 planning_duration_ms INTEGER NOT NULL CHECK(planning_duration_ms >= 0),
 prefix_comparison TEXT NOT NULL CHECK(prefix_comparison IN ('unknown','unchanged','changed')),
 created_at TEXT NOT NULL,
 CHECK((superseded_local_prediction_tokens IS NULL AND superseded_provider_input_tokens IS NULL AND superseded_counting_method IS NULL AND superseded_count_fallback IS NULL)
 OR (superseded_local_prediction_tokens IS NOT NULL AND ((superseded_provider_input_tokens IS NULL AND superseded_counting_method IS NOT NULL AND superseded_counting_method = 'conservative_local_estimate' AND superseded_count_fallback IS NOT NULL)
 OR (superseded_provider_input_tokens IS NOT NULL AND superseded_counting_method IS NOT NULL AND superseded_counting_method != 'conservative_local_estimate' AND superseded_count_fallback IS NULL)))),
 CHECK((provider_input_tokens IS NULL AND counting_method = 'conservative_local_estimate' AND count_fallback IS NOT NULL) OR (provider_input_tokens IS NOT NULL AND counting_method != 'conservative_local_estimate' AND count_fallback IS NULL))
);
CREATE INDEX request_capacity_plans_root ON request_capacity_plans(root_conversation_id,created_at);
CREATE INDEX request_capacity_plans_request ON request_capacity_plans(request_id,retry_attempt);
CREATE TABLE request_prefix_change_causes (
 plan_id TEXT NOT NULL REFERENCES request_capacity_plans(plan_id) ON DELETE CASCADE,
 ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
 cause TEXT NOT NULL CHECK(cause IN ('clearing','compaction','guidance_refresh','model_route_switch','tool_policy_change','readable_projection')),
 PRIMARY KEY(plan_id,cause),
 UNIQUE(plan_id,ordinal)
);
