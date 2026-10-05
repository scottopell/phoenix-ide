# Make GPT-6.1 Sol Standard the Phoenix ordinary default

## Commission and observed journey

The user selected GPT-6.1 Sol with Standard request speed as the new Phoenix product and ordinary-worker default everywhere reasonable. Global reports both live hosts at f5f98d7 advertise exact gpt-6.1-sol with supported speed and native medium effort, but /api/models still reports gpt-6-astra as default. Existing Global runtimes, explicit user/task/persona choices and effort overrides must remain unchanged. No live mutation is admitted to this implementation owner.

## Verified findings on fetched origin/main

- ModelRegistry::pick_default_model honors an available DEFAULT_MODEL first; its source PREFERRED list omits gpt-6.1-sol and places Claude Sonnet models before Astra. The source fallback must change, not just an operational note.
- resolve_creation_model preserves explicit model selections, uses registry default for ordinary direct creation, and picks cheap_model_id_for_provider for unspecified managed/auto-with-repo creation. This is a second ordinary creation default seam to review against the new commission; cheap auxiliary/title work remains specialized and out of scope.
- useCreateConversation selects a valid remembered explicit model before server default. Do not erase remembered selections to force adoption. Native NewConversationView.loadModels uses the advertised server default; pending creation attempts preserve their exact identity.
- Named workers/tiers are loaded solely from $XDG_CONFIG_HOME/phoenix-ide/config.toml (or $HOME/.config). There is no built-in repository coding_worker default to rewrite. SpawnCatalog::select honors task execution, then declared persona candidates, then exact parent model/connection/effort inheritance. REQ-AG-005/010/011 require these boundaries.
- DEFAULT_MODEL is loaded from .phoenix-ide.env by existing deployment tooling and persisted into controller environment snapshots. Restart/redeploy behavior must be validated against the actual host controller; editing notes or an arbitrary env file is not runtime activation.
- Standard is already the base request speed. Omitted effort uses model-native semantics; the new default must not inject a blanket medium or other effort override.
- The project coordinator independently verified both hosts have installed DEFAULT_MODEL unset, coding_worker configured as gpt-5.6-luna/codex/high, and no tiers. Activation must change that ordinary worker's model only to gpt-6.1-sol/codex while preserving its explicit high effort; validate high support before applying. These operational reads are supplied by Global, not inferred from repository source.
- The current coordination authority is roadmap Issue #806; #651 is superseded.

## Interaction map

Source fallback or available DEFAULT_MODEL -> registry /api/models.default -> fresh browser/native selection -> creation model resolution -> persisted exact conversation model/speed/effort.

Task explicit selector -> persona configured ordered execution candidates -> anonymous parent inheritance -> persisted child execution identity. Operational ordinary-worker preferences live in host config, independently of product fallback.

Repository .phoenix-ide.env -> supported deployment candidate environment -> installed controller snapshot -> startup registry. Only an actually authorized executor performs host persistence edits and activation.

## Proposed bounded implementation

1. Branch from actual latest origin/main using normal new-task workflow, preserving old merged branch history and any unowned work. Do not reopen #809.
2. Prefer exact gpt-6.1-sol when registered in the product default list; retain available explicit DEFAULT_MODEL precedence and honest missing-model behavior. Retain existing fallback order when GPT-6.1 Sol cannot be registered.
3. Apply this product default to ordinary creation without an explicit model, including managed/auto ordinary creation where the cheap-model default would otherwise defeat this commission. Update only the owning normative default policy and matching tests; preserve deliberately cheap auxiliary/title tasks and specialized chain/review choices.
4. Keep Standard as existing default speed and native effort omission; preserve valid explicit effort and explicit speed/model/task/persona choices. Do not add a settings API, new configuration framework, runtime model substitution, provider-error fallback, catalog inference, or blanket effort.
5. Verify browser remembered picks and native server-default consumption with existing focused tests/harnesses. Change client source only if evidence demonstrates a default-only defect.
6. Produce a precise Global activation checklist for both hosts using existing supported environment/config paths: persist DEFAULT_MODEL=gpt-6.1-sol in each actual deployment input and installed snapshot through qualified activation; inspect only non-secret ordinary-worker execution fields and change only designated ordinary coding worker/tier candidates to exact gpt-6.1-sol on the existing connection, preserving explicit effort and specialized workers. No live config writes by this owner. If that declared explicit effort is incompatible, report the conflict rather than silently clearing it.

## Before/after and readback acceptance

- Source tests: fresh default selects gpt-6.1-sol when available even with other providers; explicit available DEFAULT_MODEL wins; unavailable GPT-6.1 Sol leaves existing available fallback order; missing explicit model does not substitute after execution error.
- Creation tests: omitted ordinary model resolves to product default in relevant modes; explicit specialized model remains exact. Existing conversation/task identities and paused #788 stay unchanged.
- Speed/effort tests: new unspecified conversations/workers use Standard and native omission; valid explicit effort/speed choices survive. Native medium capability is not a new forced effort value.
- Browser: fresh profile/no remembered pick follows /api/models.default; remembered valid explicit model and effort remain; no mass localStorage migration. Native: server default is selected for fresh creation, pending attempts remain exact.
- Workers: omitted anonymous execution inherits parent exactly; explicit task/persona overrides remain; changed ordinary host worker config is read back through supported advertised worker execution, not inferred from documentation.
- Qualified activation by an actually authorized executor only, separately for primary and devmbp: record deployed source revision; verify non-secret DEFAULT_MODEL in actual deployment input and installed environment; qualified activate; read /api/models default=gpt-6.1-sol and exact speed/effort capabilities; verify fresh browser/native and worker advertised selections. Repeat readback after supported restart/redeploy to prove persistence. Preserve current Global runtime model/effort throughout.
- Run focused registry/creation/client/worker tests, ./dev.py check, inspect diff, commit/push one new PR, request exact-head Codex review and converge actual findings plus CI. Mark task done before handing off.

## Non-goals and authority

No live restart/deploy, production DB/conversation writes, credential dumps, paid/live model probes, new writers, old PR reopening, or interference with release #836/#842/Kache. Global owns qualified review/merge/deployment coordination; operational writes require the executor’s actual filesystem authorization. Host-specific ordinary-worker configuration facts remain unverified until Global supplies/readbacks them; do not invent a repository default or claim source publication changes live behavior.

## Qualified operational activation receipt (not executed by implementation owner)

Coordinator-supplied before values on both primary/devmbp: installed DEFAULT_MODEL unset; ordinary coding_worker gpt-5.6-luna/codex/high; tiers empty. Source publication alone is not a live activation receipt.

1. An actually authorized host executor records controller type and deployed commit per host. Persist `DEFAULT_MODEL=gpt-6.1-sol` in that deployment's actual `.phoenix-ide.env` input, without reading/publishing other environment values. Preserve all unrelated keys. Persist only coding_worker model change in the actual XDG `phoenix-ide/config.toml`: `gpt-6.1-sol`, connection `codex`, existing explicit `high`. Leave tiers empty and other worker/persona definitions untouched.
2. Qualify exact model/connection high support via advertised route capabilities. Standard remains the existing base speed; do not create an effort or speed setting framework.
3. Important controller distinction: `_launchd_candidate_env` and controller-enabled systemd/bare deployment preserve installed configuration; a `.phoenix-ide.env` edit alone does not enter those candidates. Non-controller local deploy loads that input. launchd restart preserves installed state, while bare restart re-snapshots input. Use the existing authorized deployment/configuration workflow appropriate to the host to include the desired environment in the qualified activation, and inspect only DEFAULT_MODEL in its candidate/installed snapshot. Do not patch installed plists/controller snapshots ad hoc or falsely claim a preserving deploy propagated the input. If only a preserving controller is authorized, leave DEFAULT_MODEL unset: merged source fallback already persists the new product default across such restart/redeploy; record that explicit env pin is not applied rather than inventing an unsupported config update.
4. Combine preference activation with the next qualified source deploy; no extra restart just to experiment. Read back exact deployed source, `/api/models.default=gpt-6.1-sol`, Standard/Fast support and native medium effort, and advertised ordinary coding_worker `gpt-6.1-sol/codex/high` through the supported worker catalog. Retained Global/other conversations must still report their previous explicit model/effort.
5. Fresh browser with no remembered selection follows server default, whereas existing valid remembered model/effort stays exact. Native fresh creation consumes server default; existing pending attempts retain identity. Use offline tests for this PR and authorized local acceptance without paid model probes.
6. At the next otherwise-needed supported restart/redeploy, repeat default and worker readback to establish persistence. Keep source/config evidence, runtime activation, and user acceptance separate. This owner has not performed any host config write, restart, deploy, or live conversation mutation.

Admission bookkeeping: the approval workflow used colliding 98017 and also copied the unrelated benchmark task to in-progress. The unrelated benchmark is restored to its origin/main done filename/content; `taskmd fix` assigned this follow-up unique ID 98019. The reported conversation metadata still projecting old task98012 is recorded only; no DB repair is attempted.
