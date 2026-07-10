# Build comprehensive tool-call and tool-result QA fixtures

## Context and triage

The stale artifact is `ui-audit/`, introduced once in commit `aeb8eef7` (`docs(ui-audit): commit stage 1 inventory + rubric + stage 2 pilot evals`). It is an isolated manual conversation-widget audit with no runtime or test references. Only four of its 22 planned evaluations were completed:

- `tool-call-bash`: 21/25
- `tool-call-generic` (sampled only with `read_file`): 14/25
- `tool-call-patch`: 16/25
- `tool-call-think`: 16/25

The checked-off screenshots are not present in the worktree and were never tracked in Git. The remaining tool-result pages (`text`, `image`, and `subagent-summary`) are empty placeholders.

The audit is also materially stale. Since it was written, the renderer gained dedicated `read_file`, `ask_user_question`, `search`, and `browser_*` input summaries; subtle collapsed `think` rendering; trivial-patch inline rendering; structured bash/tmux, search, keyword-search, console-log, browser-profile, image, skill, and sub-agent result views; live duration/missing-result states; and compact tool strips. Its durable value is the rubric: information density, state legibility, consistency, in-context scannability, and fidelity.

The current executable replacement is `./dev.py qa message-list`, but it has only three compact-density scenarios and captures every story at one nominal 960×900 viewport. Its tool-rich story primarily tests the compact strip, not the full tool-result renderers, and it does not automatically exercise a true desktop width and a phone width.

## Goal

Create a focused, deterministic Ladle QA surface that makes the complete current tool-call/tool-result rendering grammar easy to inspect and improve on both desktop and mobile. Prefer structured, truthful renderings and make generic JSON fallback conspicuous rather than allowing it to hide among untested paths.

## Implementation plan

### 1. Add viewport variants to the shared Ladle capture engine

Extend `runSurfaceCapture`/`captureSurface` with an optional named viewport matrix while preserving the current single-viewport behavior for existing surfaces.

For the new surface, capture every story at:

- desktop: 1280×900 (unambiguously above the conversation spec’s desktop breakpoint)
- mobile: 390×844

Use stable variant-qualified output paths or filenames so both captures coexist and are obvious in `qa-artifacts/tool-results/`. Continue to discover stories from Ladle’s manifest, wait on scenario-valued ready markers, fail on unexpected console/page errors, and take full-page screenshots.

### 2. Add a dedicated `tool-results` fixture surface

Add deterministic scenario data under `ui/src/fixtures/toolResults/`, Ladle stories, a stable ready marker, `ui/scripts/capture-tool-results.mjs`, a `qa:tool-results` package script, and `./dev.py qa tool-results` wiring.

Render through the real conversation/message components rather than a visual imitation. Use typed fixture builders for assistant `tool_use` blocks and paired tool messages so payloads remain readable and IDs/results cannot drift accidentally. Scenario metadata should declare density and intentional pairing state.

Organize screenshots by renderer family so each remains reviewable in context rather than creating one enormous wall of widgets:

1. **Lifecycle and generic fallback**
   - pending/in-flight call
   - finalized call whose result is missing
   - short success, empty success, and explicit error
   - long collapsed output, auto-expanded payload output, and visible 5,000-character truncation
   - unknown-tool input/output fallback, deliberately included as the JSON-baseline smell
   - completed duration versus active elapsed treatment (with deterministic fixture timing or a stable rendering seam; do not introduce screenshot sleeps)

2. **Input-summary grammar**
   - `bash` run/peek/wait/kill, including multiline and malformed/legacy fallback
   - `tmux`, `patch`, `keyword_search`, `read_image`, ranged `read_file`, `spawn_agents`, `ask_user_question`, and `search`
   - representative browser actions (`navigate`, eval, screenshot, console, resize, wait, click, type, key press, profile)
   - one truly unknown tool to keep raw-JSON fallback visible

3. **Bash process results**
   - running/still-running, exited success, non-zero/error, killed/tombstoned, and `kill_pending_kernel`
   - stdout/line windows, labels/handles, signal/final-cause/duration metadata
   - legacy plain-text fallback
   - inspector affordance where the real provider context permits it

4. **Tmux results**
   - stdout only, stderr only, both streams, non-zero exit/error, truncation metadata, and malformed/plain-text fallback

5. **Search-oriented results**
   - structured `search` matches, empty results, and unparseable fallback
   - filtered `keyword_search`, raw-ripgrep fallback, and empty results
   - clickable-file and non-clickable rendering where materially different

6. **Browser results**
   - console logs: levels, empty set, pointer/status text, and unparseable raw fallback
   - screenshots via current `display_data` plus the oldest parseable-JSON fallback
   - browser-profile structured families: scenario completed/blocked, metrics, heap diff, CPU summary, trace summary, missing structured payload, generic profile action, and error

7. **Images**
   - current typed `images` channel for `read_image`
   - browser screenshot `display_data`
   - legacy JSON image payload
   - malformed/non-image fallback
   - use a small deterministic local asset/base64 payload, never a live network resource

8. **Patch results**
   - trivial single patch rendered inline
   - multi-file diff with summary/open-file affordance
   - failed patch and legacy diff-in-text fallback
   - enough long paths and line counts to expose mobile wrapping

9. **Skill, sub-agent, and proposal results**
   - skill loading/success/failure, source/snippet, and static/clickable source treatment
   - sub-agent running/success/failure/timed-out outcomes and expandable long summaries
   - fork-proposal review affordance when supported by the fixture providers; otherwise cover its derivation in a targeted component test rather than faking the visual state

10. **Conversation-density stress cases**
    - full-density mixed success/error stack for the old audit’s “in-context scannability” criterion
    - compact-density tool strip containing repeated tools, mixed statuses, long summaries, and mobile wrapping
    - retain the existing message-list compact fixture for its current regression purpose; share builders/data where useful rather than duplicating payloads

Each family must include at least one error or fallback state where meaningful. Both viewport variants capture every story so mobile coverage cannot silently lag desktop coverage.

### 3. Add targeted fixture and renderer tests

Add tests that provide structural protection beyond screenshots:

- scenario IDs are unique and every declared scenario has a story
- every `tool_use` has exactly one paired result unless explicitly marked pending or missing
- fixture payloads cover each specialized input formatter and each specialized result dispatch family
- the generic/unknown fallback remains an explicit fixture, not an accidental omission
- scenario density and theme values are valid and deterministic
- expansion/collapse, copy, file-open/inspect affordances, and error/fallback semantics receive focused component tests where a static screenshot cannot prove behavior
- viewport-variant output naming/configuration is tested as a pure derivation if capture-engine logic is extracted

Do not duplicate exhaustive parser assertions already present in `MessageComponents.test.tsx`, `BrowserProfileResponseView.test.tsx`, and the search-output tests; use fixtures for visual composition and add tests only for uncovered behavior/contracts.

### 4. Retire the stale manual audit

After the executable fixture coverage lands, remove `ui-audit/` and its screenshot ignore rule. Do not preserve empty placeholder evaluations. Git history retains the four pilot reviews; their still-relevant criteria are represented by the lifecycle, error/fallback, density, desktop/mobile, and in-context scenarios above.

If implementation reveals a genuine product rendering defect (rather than only a fixture gap), fix adjacent low-risk issues with focused tests. Record larger renderer redesigns as follow-up work instead of expanding this QA-infrastructure task without bound.

## Validation

- Run focused Vitest suites for fixture structure and touched renderers.
- Run `cd ui && pnpm exec tsc --noEmit`.
- Run `./dev.py qa tool-results` and inspect all desktop and mobile artifacts for overflow, unreadable wrapping, hidden errors, silent truncation, and accidental raw JSON.
- Verify `./dev.py qa --help` exposes `tool-results`.
- Run `./dev.py check`.

## Acceptance criteria

- One command captures every tool-result fixture on true desktop and phone viewports.
- Every specialized renderer branch in `MessageComponents`/`BrowserProfileResponseView` is represented by at least one deterministic scenario, alongside pending, missing, error, malformed, empty, long/truncated, and unknown-tool fallbacks.
- Full and compact conversation density are both represented in realistic stacks.
- Scenario/story/result-pairing tests make coverage drift obvious.
- Existing QA capture surfaces remain backward compatible.
- The obsolete `ui-audit/` artifact is removed after its useful intent is superseded.
