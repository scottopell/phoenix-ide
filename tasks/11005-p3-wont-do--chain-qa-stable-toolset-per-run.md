chain_qa::build_agent_request gates the search_conversations tool on per-turn index freshness (the search_enabled flag), while every Q&A turn shares one stable cache key per language (PromptCacheKey::stable("chain-qa-agent/<language>")). When index freshness flips mid-run, the tool set -- part of the cached prompt prefix -- changes between turns of the same agent run.

Same bug class as the Explore taskmd ID hint instability (specs/chains, crates/phoenix-ide/src/chain_qa.rs): prompt prefix content recomputed from live mutable state per request. Two effects:

1. Prompt cache busted at the tools block on every flip, for every chain-qa run sharing that key.
2. The model can see search_conversations one turn and have it silently vanish the next; a call to the vanished tool fails confusingly mid-answer.

Fix direction: snapshot search_enabled once per Q&A invocation (in prepare_invocation, alongside the skeleton/snapshot it already takes) and hold it for the run's lifetime, mirroring how the Explore ID hint was stabilized. A run that starts without search keeps read_conversation only; freshness changes apply to the next submission.

Acceptance: within one Q&A run, the tool set offered to the LLM is identical across turns (force_answer final turn excepted -- that empty tool set is intentional); add a request-shape regression test mirroring explore_prompt_cache_shape_tests.

## Resolution: wont-do

The premise does not hold. `run_answer_invocation` calls `await_index_fresh`
once per invocation and binds the result for every planning turn, so the tool
set offered to the LLM is already stable for the run's lifetime. Mid-run index
staleness is handled at execution time inside `execute_tool` with an explicit
tool-result error ("search unavailable: the index is not current..."), not by
removing the tool from the declaration. That is the correct shape: stable
prompt prefix, explicit runtime refusal.

The residual cache observation (two possible tool sets sharing the
`chain-qa-agent/<language>` cache key across *different* invocations) is a
cohort-design choice, not prefix instability within a conversation -- the bug
class this task was filed against.
