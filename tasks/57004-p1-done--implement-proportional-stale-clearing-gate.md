# Implement proportional stale tool-result clearing gain gate

## Goal

Prevent stale tool-result clearing from degenerating into repeated low-yield cache-busting sweeps in long patch-heavy sessions.

The current planner uses an absolute worthwhile-gain gate (`CLEAR_AT_LEAST_TOKENS`, currently 8192). In the pathological tail state, this allows a sweep that frees only ~8k tokens while invalidating a ~190k-token prefix, which is especially expensive on OpenAI-style automatic prefix caching.

Implement Option 1 from the cache-ratchet investigation: scale the worthwhile-gain gate to the re-encode cost.

```text
min_required_gain = max(CLEAR_AT_LEAST_TOKENS, current_prompt_tokens * gain_fraction)
```

A sweep should advance the clear watermark only when the eligible freed tokens are large enough relative to the prompt/prefix being disturbed.

## Prerequisite

First complete the modeling artifact from:

- `tasks/57003-p2-ready--model-stale-clearing-proportional-gate.md`

Use that artifact to lock in the intended behavior before changing runtime code:

- patch-heavy ratchet preset: proportional gate holds once freeable gain becomes too small
- short clean-win preset: proportional gate still allows a large useful sweep
- read/search-heavy preset: clearable-heavy sessions still benefit

If the model shows the proposed rule needs different inputs or a different default fraction, update this task’s implementation plan before coding.

## Scope

Implement the proportional gain gate in the stale tool-result clearing planner.

Likely areas:

- `crates/phoenix-ide/src/runtime/executor.rs`
  - `plan_tool_result_clearing`
  - stale tool-result clearing constants/tests
- `specs/stale-tool-results/`
  - update requirements/design/executive docs as needed so the gain gate is specified, not accidental

## Design constraints

The implementation must preserve existing stale-clearing invariants:

- Never clear non-re-queryable tool results.
- Never mutate durable `db::ToolContent`; clearing remains request-rendering only.
- The clear watermark remains monotonic.
- The cleared set only grows.
- The recency floor is preserved.
- The planner must remain provider-agnostic unless the modeling artifact demonstrates that provider-specific tuning is required.

Do not make `patch`, `ask_user_question`, `think`, `submit`, `spawn_agents`, or `skill` broadly clearable as part of this task. Result-level clearability is a separate follow-up.

## Implementation plan

1. Complete and review the modeling artifact from task 57003.
2. Choose a default proportional gain fraction from the model.
   - Prefer a conservative fraction that eliminates the low-yield ratchet without suppressing clean large sweeps.
   - Document the rationale in the spec, not as a distributed code comment.
3. Change the planner’s worthwhile-gain test from absolute-only to proportional:

   ```text
   eligible_freed >= max(CLEAR_AT_LEAST_TOKENS, prompt_tokens_for_pressure * PROPORTIONAL_GAIN_FRACTION)
   ```

4. Ensure the pressure/prompt value used by the gate matches the planner’s pressure signal and is stable/testable.
5. Add or update unit tests covering:
   - current clean sweep still advances when eligible freed tokens are large
   - watermark holds when eligible freed is above 8192 but below the proportional threshold
   - patch-heavy/unclearable-heavy synthetic ratchet does not advance repeatedly with low-yield sweeps
   - monotonic watermark and recency-floor behavior remain intact
6. Update `specs/stale-tool-results/` to describe the proportional worthwhile-gain rule and its purpose.
7. Run appropriate validation:
   - targeted Rust tests for stale tool-result clearing
   - broader `./dev.py check` if feasible

## Acceptance criteria

- The runtime planner uses a proportional gain gate rather than only the absolute 8192-token floor.
- A low-yield sweep that frees ~8k tokens on a ~190k prompt is rejected by default.
- A large first sweep in a short/clearable-heavy session is still accepted.
- Existing invariants around clearability, durable content, monotonic watermark, and recency floor remain tested and passing.
- Specs describe the proportional gain rule and why it exists.
- The modeling artifact remains in-tree as review evidence for the chosen threshold.

## Follow-up, out of scope

- Result-level clearability, especially assessing whether old `patch` result bodies can be safely cleared while retaining the tool-use fact.
- Provider-specific tuning for OpenAI vs Anthropic.
- Hysteresis or minimum-turns-between-sweeps.
- Full counterfactual billing model over production traces.

## Context

### Primary concerns from this conversation

- The originally supplied SHA `03f2a76a` was a mis-paste and should not be treated as causally related to stale tool-result clearing. It is the grounding-panel redesign commit.
- The relevant stale-clearing merge is `b711271a feat: stale tool-result clearing behind a cache-stable watermark (#330)`, with implementation ancestry including `bf3557c7`, `c4e8c79e`, `58df2f3a`, `0413f7c7`, `35dfb1d1`, `8212c43f`, and `9ee8bca1`.
- The desired fix is specifically Option 1 from the investigation: make the worthwhile-gain threshold proportional to prompt/re-encode cost, not merely the existing absolute `8192` token floor.
- Before implementing runtime behavior, keep task 57003 in-tree as a prerequisite modeling artifact. The artifact should build intuition and help choose/validate the proportional threshold.
- The implementation task should not expand tool clearability, especially not broad-clearing `patch`, `ask_user_question`, `think`, `submit`, `spawn_agents`, or `skill` results. Result-level clearability is a separate design-sensitive follow-up.

### My handoff notes from prod DB analysis

Prod DB inspected read-only at `/Users/scottopell/.phoenix-ide/prod.db`.

Scale and provider shape:

- 174 conversations.
- 173 conversations with usage rows.
- 10,212 `turn_usage` rows at time of inspection.
- Usage range: `2026-05-03T19:52:07.218617+00:00` to `2026-06-29T02:25:35.799334+00:00`.
- `cache_creation_tokens != 0` rows: 0.
- Conversations with `clear_watermark > 0`: 7.
- Model mix is overwhelmingly `gpt-5.5`; observed cache behavior is OpenAI-style cache-read-only accounting.

Aggregate before/after Jun 20 is confounded and should not be used as causal evidence:

- Before Jun 20: 5,539 turns, 56.5% hit rate, 250,472,524 uncached input, 325,150,720 cache read.
- After Jun 20: 4,673 turns, 73.1% hit rate, 126,401,877 uncached input, 343,110,656 cache read.
- Early weeks had 0% hit rate, so the aggregate before/after trend hides the tail regression.

Watermark-tail evidence supports the ratchet concern:

- Only 7 conversations advanced `clear_watermark`, so this is a long-session tail issue rather than broad degradation.
- `add-phoenix-native-commission-review-tool` confirmed the attached handoff’s key numbers:
  - 828 turns.
  - 18 inferred prompt-drop sweeps.
  - First drop: `192,015 -> 54,009`, freed ~138,006.
  - Last drop: `200,551 -> 192,522`, freed ~8,029.
  - Front half: 81.8% hit, 22,535 avg uncached input/turn, 123,691 avg prompt.
  - Back half: 66.6% hit, 58,340 avg uncached input/turn, 174,692 avg prompt.
  - Back-half uncached input/turn was ~2.59x the front half.
- Other watermark conversations showed similar back-half uncached-input amplification:
  - `conversation-sidebar-delete-refresh-bug-2`: ~2.64x.
  - `saturday-morning-sunset-thunder-2`: ~1.65x.
  - `add-cached-pr-badges-to-conversation-sidebar`: ~2.79x.
  - `archived-conversations-should-render-read-only-in-the-ui`: ~1.46x.
  - `review-sandbox-branch`: ~2.75x.
  - `redesign-mobile-conversations-list-around-current-work-chains-and-pr-relevance`: ~1.97x.
- Several affected conversations had sustained low-hit runs on large prompts, especially `add-phoenix-native-commission-review-tool`, `review-sandbox-branch`, and `redesign-mobile-conversations-list-around-current-work-chains-and-pr-relevance`.

Interpretation:

- Shorter or clearable-heavy sessions can still benefit from a large first sweep.
- The pathological behavior emerges when unclearable results, especially patch-like state-changing records, form a growing permanent prefix while the clearable layer below the recency floor becomes thin.
- The existing absolute threshold allows tiny late sweeps that disturb a large cached prefix. On OpenAI-style prefix caching, repeated mid-history changes depress steady-state cache hit rate and increase full-price input.
- The proportional gate should reject low-yield sweeps such as freeing ~8k on a ~190k prompt while preserving large early sweeps.

### XML handoff from preliminary findings

The following is the attached preliminary handoff that motivated this task. It is included here so future implementers do not need the original conversation attachment.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<agent-handoff>
  <meta>
    <title>Stale tool-result clearing: cache-churn ratchet on the OpenAI/Responses path</title>
    <spec>specs/stale-tool-results/ (REQ-STR-001..011)</spec>
    <feature-pr>#330 — feat: stale tool-result clearing behind a cache-stable watermark</feature-pr>
    <related-tasks>
      tasks/94003-p2-ready--stale-tool-result-clearing.md (the feature)
      tasks/58040-p2-ready--per-result-clearability.md (deferred; directly relevant to the fix)
      tasks/58039-p3-ready--clearing-pressure-usage-ordering.md (deferred; secondary)
    </related-tasks>
    <evidence-db>snapshot-of-prod.db (post-migration 030; has clear_watermark column)</evidence-db>
    <provider-scope>
      Prod DB is ~100% OpenAI (gpt-5.5 dominant; also gpt-5.4, gpt-5.4-mini, gpt-5.3-codex).
      Zero Anthropic/Claude turns present. cache_creation_tokens = 0 on every row
      (OpenAI has no cache-write token concept). Provider asymmetry below is reasoned,
      not measured.
    </provider-scope>
    <objective>
      Investigate, then PROPOSE A FIX for a positive-feedback loop: in long
      patch-heavy sessions the clearing sweep degenerates from rare/spaced into
      near-every-turn full-price prefix re-encoding. Do not just confirm — design
      a remedy that holds the REQ-STR invariants.
    </objective>
    <constraints>
      Any fix MUST preserve: REQ-STR-002 (never remove non-re-queryable results),
      REQ-STR-005 (durable record untouched — db::ToolContent never mutated),
      REQ-STR-007 (watermark monotonic; cleared set only grows; recency floor kept).
      Do NOT make patch/ask_user/think/submit clearable at the tool level — their
      results are the sole record of a state-changing effect or a human answer.
    </constraints>
  </meta>

  <problem-statement>
    Reported symptom: on the OpenAI/Codex path a user felt quota was "consumed way
    longer" after this feature shipped (~2026-06-20). Hypothesis tested and CONFIRMED:
    a self-reinforcing ratchet drives the clearing sweep into a high-frequency,
    low-yield, cache-busting regime on long sessions.

    Mechanism (one sentence): unclearable tool results — chiefly `patch`, plus
    think/spawn_agents/skill — accumulate into a permanent prefix the watermark can
    never shave; as that baseline ratchets toward the clear_trigger, each sweep can
    only clear the thin clearable layer between the unclearable baseline and the
    3-round recency floor, so sweeps fire ever more often and free ever less, each
    one re-encoding a ~190k-token prefix at full price. The spec frames per-turn
    sweeping as a "bounded exception under sustained pressure" (design.md, last
    bullet of "Interaction with prompt caching"); in a long patch-heavy coding
    session it is the STEADY STATE, not the exception.
  </problem-statement>

  <why-openai-makes-it-worse>
    Clearing edits a tool result in the MIDDLE of history (below the recency floor,
    not at the tail), moving the first-differing token earlier. This invalidates the
    cached prefix from that point on ANY prefix-hash cache — both providers.

    The asymmetry is in the cost model, not the invalidation:
    - Anthropic: explicit breakpoints + a cache-WRITE tier (~1.25x input once, then
      reads at ~0.1x). A sweep pays a bounded write, then warm reads amortize it.
    - OpenAI Responses (openai.rs): pure automatic prefix caching keyed on
      prompt_cache_key = conversation id; NO cache-write token. The first occurrence
      of a changed prefix is billed at FULL input price with no discount; only a
      later byte-identical request reads cheap. Near-every-turn sweeping denies that
      "later identical request," so the re-warm rarely happens and full-price
      re-encodes repeat. The design's "pay once per sweep, re-warm between"
      amortization fails harder here.

    This is the kernel of truth in the original "overfit to Anthropic caching"
    intuition: the economics that make spaced sweeps tolerable are more forgiving on
    Anthropic; the same churn is structurally more expensive on OpenAI.
  </why-openai-makes-it-worse>

  <empirical-findings>
    <scale>
      174 total conversations; 172 actually ran; only 7 ever advanced the watermark.
      This is a TAIL phenomenon affecting the longest sessions — exactly where a user
      notices "quota lasted longer."
    </scale>

    <aggregate-caution>
      Overall cache-hit rate "56.5% before vs 73.5% after 06-20" is CONFOUNDED — early
      weeks (W18-20) show 0% hit because caching was effectively off then, not because
      of this feature. Do not cite the before/after as evidence the feature helped.
    </aggregate-caution>

    <case-study conversation="add-phoenix-native-commission-review-tool" model="gpt-5.5" turns="828">
      <trigger-inference>
        Sweeps fire at prompt ~190-200k => configured context_window ~272k, trigger
        = 7/10 * window ~= 190k. (CLEAR_TRIGGER_NUM/DEN = 7/10 in executor.rs.)
      </trigger-inference>

      <ratchet>
        Post-sweep landing size (the floor a sweep drops to) RISES every sweep:
          sweep  0 @turn 137: 192,015 -> 54,009   (freed 138,006)
          sweep  2 @turn 396: 192,336 -> 116,238  (freed 76,098)
          sweep  5 @turn 605: 191,714 -> 153,060  (freed 38,654)
          sweep 10 @turn 746: 206,468 -> 180,039  (freed 26,429)
          sweep 17 @turn 815: 200,551 -> 192,522  (freed 8,029)
        Post-sweep floor: 54k -> 192k (3.6x). Freed-per-sweep: 138k -> 8k (right at
        the CLEAR_AT_LEAST_TOKENS=8192 gate, which is the ONLY thing preventing a
        literal every-turn sweep).
      </ratchet>

      <cadence-collapse>
        Turns between sweeps collapse: 179 -> 80 -> 84 -> 55 -> 70 -> 38 -> 36 -> 29
        -> 21 -> 17 -> 8 -> 15 -> 11 -> 7 -> 12 -> 10 -> 6. End of session sweeps
        every ~6-10 turns.
      </cadence-collapse>

      <billing-impact>
        Front half: hit 81.8%, avg full-price input 22,535/turn, avg prompt 123,691.
        Back half:  hit 66.6%, avg full-price input 58,340/turn, avg prompt 174,692.
        => ~2.6x more full-price input per turn in the back half. That is the measured
        "quota burned faster."
        (Note: "uncached input on sweep turns = 6% of total" UNDERSTATES the cost —
        the real penalty is the depressed steady-state hit rate after each bust, which
        the front/back half hit-rate delta captures, not just the sweep turn itself.)
      </billing-impact>

      <root-cause-composition>
        Tool calls in this conversation:
          read_file 239 (clearable), patch 227 (UNclearable), bash 198 (clearable),
          search 121 (clearable), think 10 (UNclearable), spawn_agents 2 (UNclearable),
          skill 1 (UNclearable).
        558 clearable vs 240 unclearable calls. `patch` is the 2nd-heaviest tool and is
        unclearable by design (its result records that a change was applied — an event,
        REQ-STR-002). The 240 unclearable results are the permanent prefix that ratchets
        the baseline up.
      </root-cause-composition>
    </case-study>

    <generalization>
      Same shape (single huge first sweep, then rising-floor multi-sweep churn, with
      sustained runs of >=4 consecutive low-hit turns on >80k prompts) appears in the
      other long watermark conversations: review-sandbox-branch (550 turns, churn runs
      up to 8), redesign-mobile-conversations (247 turns, runs up to 10),
      archived-conversations (452 turns). Short watermark conversations (~200 turns)
      get exactly ONE clean win-sweep and never ratchet — so the loop is length-gated.
    </generalization>
  </empirical-findings>

  <code-map>
    <entry>crates/phoenix-ide/src/runtime/executor.rs : dispatch_llm_request ->
      assemble_cleared_messages (provider-agnostic; runs BEFORE provider translation)</entry>
    <planner>executor.rs : plan_tool_result_clearing — computes floor_first_round =
      (max_round+1) - KEEP_RECENT_ROUNDS; eligible_freed over clearable, &gt;prior_watermark,
      round &lt; floor; maximal sweep to floor_boundary when over_pressure AND
      eligible_freed &gt;= CLEAR_AT_LEAST_TOKENS.</planner>
    <facts>executor.rs : collect_tool_result_facts — joins tool_use name to result by
      tool_use_id; clearable = clearable_tool_names.contains(name).</facts>
    <renderer>executor.rs : render_messages — placeholders cleared results, keeps tool_use
      block, never mutates db::ToolContent.</renderer>
    <constants>executor.rs : KEEP_RECENT_ROUNDS=3, CLEAR_TRIGGER_NUM/DEN=7/10,
      CLEAR_AT_LEAST_TOKENS=8192, IMAGE_TOKEN_ESTIMATE=1500.</constants>
    <pressure-signal>crates/phoenix-db/src/lib.rs : get_last_turn_prompt_tokens =
      input_tokens + cache_read_tokens + cache_creation_tokens of most recent turn_usage.</pressure-signal>
    <openai-cache>crates/phoenix-llm/src/openai.rs : translate_to_responses_request sets
      prompt_cache_key = request.cache_key (PromptCacheKey::stable(conv_id)); usage parse
      splits cached_tokens out of input_tokens, cache_creation_tokens=0.</openai-cache>
    <clearable-capability>crates/phoenix-tools/src/lib.rs : Tool::clearable() default false;
      ToolRegistry::clearable_tool_names(). Opt-ins: read_file, bash, search, keyword_search,
      read_image, browser screenshot/console, tmux, tmux_run, terminal history (2).
      NOT opted-in: patch, ask_user_question, think, propose_task, submit_result/error,
      spawn_agents, skill.</clearable-capability>
    <cache-key>crates/phoenix-core/src/domain/llm_types.rs : PromptCacheKey::stable/ephemeral.</cache-key>
  </code-map>

  <fix-directions ordered="by-leverage">
    <option n="1" title="Scale the worthwhile-gain gate to the re-encode cost">
      CLEAR_AT_LEAST_TOKENS=8192 is absolute, so a sweep that frees 8k is allowed to bust
      a 190k prefix — a terrible trade, especially on OpenAI with no cache-write cushion.
      Make the gate proportional to the prefix being invalidated, e.g. require
      eligible_freed >= max(CLEAR_AT_LEAST_TOKENS, k * current_prompt_tokens). This alone
      should kill end-of-session churn: once freeable tokens fall below the proportional
      bar, the watermark holds and the session rides up to the summarization tier (the
      intended last resort) instead of thrashing. Verify it does not regress the clean
      single-sweep win on short sessions.
    </option>
    <option n="2" title="Result-level clearability (land task 58040)">
      The ratchet is mostly patch results. Tool-level clearable() is too coarse. Thread a
      runtime clearable flag on the tool RESULT so the safe majority clears while genuine
      state-change records stay. For patch specifically: assess whether an OLD patch
      result body (the diff text / apply confirmation) is re-queryable in practice — the
      applied state is observable via read_file, so the diff snapshot may be a low-value
      stale snapshot like other reads, while the FACT of application is carried by the
      paired tool_use block (always kept). If so, patch results become clearable without
      violating REQ-STR-002's "sole record of a state-changing effect," because the
      tool_use block remains the record. This is the highest-fidelity fix but the most
      design-sensitive — write it up against REQ-STR-002 before coding.
    </option>
    <option n="3" title="Per-provider tuning">
      Because OpenAI has no cache-write tier, give it a higher clear_trigger fraction
      and/or larger keep_recent_rounds than Anthropic, so it sweeps later and less often.
      Cheap, but treats the symptom, not the ratchet.
    </option>
    <option n="4" title="Cap sweep frequency / hysteresis">
      Add a minimum-turns-between-sweeps or require pressure to exceed trigger by a margin
      (hysteresis band) before re-advancing, so a just-swept conversation must genuinely
      re-accumulate before paying another bust. Bounds worst-case churn but can let usage
      drift higher between sweeps.
    </option>
    <recommendation>
      Start with Option 1 (proportional gate) — smallest change, directly breaks the
      low-yield bust, provably bounded. Then evaluate Option 2 for patch as the durable
      fix to the permanent-prefix growth. Options 3/4 are mitigations, not cures.
    </recommendation>
  </fix-directions>

  <verification-plan>
    <item>Re-run all three scripts after a fix: the rising-floor table must flatten or the
      sweep count on long sessions must drop sharply, with NO sweep that frees less than
      the proportional bar.</item>
    <item>Add a planner unit test in executor.rs stale_tool_result_clearing_tests that
      reproduces the ratchet synthetically: N rounds where unclearable (patch-like) results
      dominate the pre-floor region; assert the watermark HOLDS once freeable gain falls
      below the proportional gate (rather than advancing every turn).</item>
    <item>Confirm REQ-STR-007 monotonicity and recency-floor invariants still hold (existing
      tests: maximal_sweep_clears_to_floor_then_holds, never_clears_unclearable_tool).</item>
    <item>Spot-check that short sessions still get their single clean win-sweep (no
      regression in the common good case).</item>
  </verification-plan>

  <observed-output reference="for the agent to diff its reproduction against">
    SCALE: 174 conversations, 172 ran, 7 watermark&gt;0.
    Case study floor: 54,009 -> 192,522 (3.6x); freed/sweep 138,006 -> 8,029.
    turns-between-sweeps: [179,80,84,55,70,38,36,29,21,17,8,15,11,7,12,10,6].
    Front half hit 81.8% (22,535 uncached/turn) vs back half 66.6% (58,340/turn).
    Tool mix: read_file 239, patch 227 (UNclearable), bash 198, search 121,
    think 10, spawn_agents 2, skill 1 => 558 clearable vs 240 unclearable.
  </observed-output>
</agent-handoff>
```
