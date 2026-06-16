Tier B usage metrics for the /usage page — the ones that need new backend
instrumentation and persistence (Tier A, derivable from the existing
turn_usage table, shipped in feat/usage-page).

Each of these requires capturing data we do not record today, so they are
forward-only (historical sessions stay blank):

- Time-to-first-token and tokens/sec: capture streaming timing per LLM call.
- 429s, retries, backoff time burned: retries are classified at runtime
  (ErrorKind::RateLimit) but never persisted. Add a per-attempt record.
- Context-window utilization per turn: computed at runtime
  (usage.context_window_used vs ExecutionContext.context_window) but not
  stored. Add a column to turn_usage or a sibling table.
- Compaction / continuation / truncation events: the 70% continuation
  trigger and 128KB output truncation fire with no metric. Count them.
- Cost per completed vs failed/abandoned task: join task outcome (ConvState)
  to the priced cost from the pricing module.
- Turns-to-completion distribution: derive from conversation state + turn
  rows once outcomes are tracked.

Wire each into the existing UsageOverview payload (api/usage.rs) and the
/usage page once captured. Pricing/cost helpers already exist in
crates/phoenix-ide/src/llm/pricing.rs.
