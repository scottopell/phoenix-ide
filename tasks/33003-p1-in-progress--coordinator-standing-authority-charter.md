# Coordinator standing Authority tools

## Authorized scope

Implement local Coordinator-only `read_authority` and `set_authority` tools,
backed by one current natural-language instructions document. Read it into every
ordinary Coordinator model request and the prompt inspector independently of
transcript history and continuation. Instruct Ember to interpret user feedback,
clarify material ambiguity, present the complete replacement and obtain human
approval in ordinary conversation before calling the setter.

The user's scope decision on 2026-11-04:

> Okay the human grants permission (and my ideas around auto dismissing certain states) are clearly scope creep and potentially contain complexity-cliff-review, so we must be careful here. Please consider those as a separate design problem, let's proceed with the authority tools and rely on the agent checking for human approval before calling the tool.

The approval requirement is model guidance. No server proof, `human_approved`
flag, approval card, proposal/revision protocol, timed wait, background workflow,
new conversation state, remote target, or semantic action-permission engine is
part of this implementation. The user approved implementation of the tools;
that does not seed production Authority or authorize disk deletion/deployment.

## Normative contract and decision

- `specs/coordinator-authority/requirements.md`: REQ-AUTH-001–006.
- `specs/global-recall/requirements.md`: REQ-GR-007/013/022.
- ADR-221: conversational approval and direct read/set tools.
- Existing permissions and compatibility contracts remain binding.

One instance-wide document is stored as one indivisible scalar under the
`coordinator_authority` application setting. Reuse `app_settings`; no migration
or separate current/history representation is needed. `read_authority` accepts
`{}`; `set_authority` accepts exactly `{instructions: string}`. Replace the
whole value, empty text clears it, maximum 16,384 UTF-8 bytes, explicit errors
rather than clipping. A current-Coordinator check is required at reads and
inside the committing write transaction. Existing owned-write and local
persistence-classification rules apply; conversational approval is separate
from database correctness.

## Delivery phases

- [x] Contract: narrow this task, preserve deferred research separately, author
  Authority requirements and executive, link Coordinator capability/continuation
  requirements, record ADR-221 and run task/spec preflight.
- [ ] Storage and tools: typed document validation, existing scalar persistence,
  current-source guards, closed schemas, full-replacement and clearing behavior,
  commit-time source validation and focused boundary tests.
- [ ] Runtime and prompt context: fresh shared Authority rendering in normal
  Coordinator requests and inspector; visible read failures; explicit empty
  context; conversational approval and in-scope-action policy; registry isolation.
- [ ] Qualification and integration: focused storage/tool/runtime tests, relevant
  `./dev.py check` lanes, spec preflight, executive evidence and completion status.

## Observed problem and evidence

Read-only production investigation on 2026-11-03 used deployed source ce4b0bff
and primary user messages. Source implementation work is on
`design/coordinator-authority` based on origin/main 64a6f23d0. The original old
checkout allocated provisional task 33001, already used upstream; taskmd
reallocated this task 33003. The full historical evidence and superseded larger
proposal are preserved in task 33004.

- Primary user input 6c01967a-6c9b-4b70-973b-b761cc13761e in transcript
  ee39d71c-4828-44bc-818b-1db991d84fcf, 2026-10-27T00:29:30Z, says:
  "approved, I want to issue blanket approval, this is cache and easily removable".
- Ember response 7aa23018-caeb-4b72-a732-3c671edaa056 reports clearing additional
  caches under that standing permission. It did recognize the approval.
- Later individual file/batch approvals 3d9fe394-6729-4522-8244-858b354acfca
  and 1d032178-4ce5-466e-bda4-433551e9731a show recurring scope reconstruction.
  Intervening human restrictions were not exhaustively adjudicated; do not
  import a grant from this sample.
- Existing prompts ask for confident action yet prohibit invented blanket
  permission. Explicit saved Authority must be distinguished from historical
  claims; deliberately user-granted broad authority is allowed.
- Coordinator home notes are historical evidence, not authoritative instructions.
  Summary retention alone cannot establish a current standing document.

## Complexity-cliff review

Highest cost relative to immediate value:

1. **Server-verified human grants.** An approval button looks small, but exact
   reviewed content requires proposal identity, pending lifecycle, activation,
   stale-state handling and client actions. This changes the feature into an
   interaction workflow spanning persistence, runtime, web and iOS. Lower-cost
   alternative: agent obtains conversational approval, then calls a plain setter.
2. **Automatic dismissal and late answers.** A ten-minute wait looks like a timer,
   but restart-safe continuation, Stop races, current-owner delivery and answers
   while working require durable scheduling/admission and UI independent of
   execution state. This is a separate class of runtime concern. Lower-cost
   alternative: retain existing question behavior for this feature.
3. **Distributed/versioned Authority.** Per-host replication, proposal history,
   revision conflicts and semantic action enforcement seem like refinements but
   introduce synchronization, conflict and policy subsystems. Lower-cost
   alternative: one local current document; existing tool/transcript records
   expose edits, without promising an independent policy history.

The hidden commitments are permanent pending-request state, crash-spanning
execution after a deadline, hard human-consent proof and cross-instance policy
synchronization. Removing them collapses this change to bounded document CRUD
and prompt assembly. The remaining source guard, input validation, atomic local
write and fresh read are ordinary correctness obligations. Do not add boolean
approval theater or a generic permission framework to recover the removed scope.

## Acceptance and validation

- [ ] Read default empty Authority; replace and read exact text; empty setter
  clears it; file-backed reopen retains it without transcript-summary recovery.
- [ ] Reject malformed arguments, extra fields and more than 16,384 UTF-8 bytes;
  boundary-byte behavior is correct for multibyte text and never clips.
- [ ] Deny ordinary conversations, sub-agents and stale Coordinator predecessors,
  including a source that rotates before its write commits.
- [ ] Register both tools only for the Coordinator; no remote selectors or
  model-supplied source identity.
- [ ] Writes follow existing owned-command outcome classification and do not
  publish ordinary success/error after unclassified local authority.
- [ ] Live provider requests and prompt inspector expose the same current body.
  Setting Authority changes subsequent requests without a runtime restart.
- [ ] Watch-driven and successor Coordinator requests receive current Authority;
  unchanged text renders identically; failed reads remain visible failures.
- [ ] Instructions require interpreted scoped wording and conversational approval
  before setting, preserve later user constraints, distinguish one-shot approval,
  and direct routine in-scope work without renewed permission.
- [ ] Prompt checks establish what instructions are sent, not guaranteed model
  compliance. No live production policy or disk-cleanup experiment is required.
- [ ] Run appropriate focused tests, `./dev.py check`, task/spec validation and
  final diff review; no UI/codegen work unless an actual typed wire change occurs.

## Separate design problem

Task 33004 (`durable-human-interactions-and-bounded-waits`, brainstorming) retains
the full prior investigation of formal approval, independently pending questions,
wait deadlines and late answers. It is not a delivery dependency, a mandatory
rollout phase or an implementation commission. Existing AUQ documentation drift
and dormant fork-proposal code observations stay with that research; this task
does not redesign or repair those paths.
