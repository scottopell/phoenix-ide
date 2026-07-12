# Wake Contracts

## User Story

As an LLM agent, when I have started a long-running bash command or tmux-backed
command and have nothing useful to do until that handle reaches a terminal
outcome, I need a way to tell Phoenix "wake me when this handle resolves"
without spending conversation turns on polling. Phoenix must durably record that
request, observe the runtime until a terminal outcome is known, and deliver
exactly one continuation into the owning conversation.

## Scope

Wake contracts are Phoenix's durable terminal-wait plane for concrete runtime
handles. This specification covers only bash handles and tmux-run window handles.
It excludes general actor messaging, request/reply continuation, webhook wake,
file/port watchers, and sub-agent wake semantics.

A wake contract is conversation-scoped: it belongs to exactly one conversation at
any moment and resolves by resuming exactly one conversation. Handle survival is
a separate concern from wake ownership transfer.

## Requirements

### REQ-WAKE-001: Receipt-Then-Runtime-Observation Registration

WHEN an agent calls `wait_until { handle, max_wait_seconds? }`
THE SYSTEM SHALL durably persist the wake contract before any runtime observation
begins

WHEN registration succeeds
THE SYSTEM SHALL return a receipt immediately after persistence, without waiting
for runtime completion, and SHALL leave the conversation's core runtime state
unchanged

IF tool cancellation is observed before a registration receipt becomes visible
THE SYSTEM SHALL leave no pending wake contract or auto-resume obligation for that
registration; a pending contract committed during the cancellation race SHALL be
atomically terminalized as cancelled before the registration returns

IF registration atomically commits an immediate terminal observation before tool
cancellation is observed
THE SYSTEM SHALL atomically choose either cancellation with a non-auto-resuming
cancelled observation, or successful registration with the persisted terminal receipt;
THE SYSTEM SHALL NOT report cancellation while retaining a hidden auto-resuming
terminal observation

IF that terminal observation has already been materialized or accepted when
post-commit cancellation is serialized
THE SYSTEM SHALL preserve the terminal registration and return its receipt because
cancellation can no longer truthfully hide delivery

A completed tool round SHALL park instead of requesting the LLM only while at least
one wake contract registered by that round remains pending at the parking decision;
terminal contract history and transient success markers SHALL NOT authorize parking

WHEN a completed tool round contains both successful wake registration and
outstanding sub-agent fan-in
THE SYSTEM SHALL durably checkpoint the complete tool round, remain awaiting the
sub-agents, and retain park intent associated with that round's registration
identities

WHEN the final sub-agent result for such a round is durably persisted
THE SYSTEM SHALL persist the conversation as idle instead of requesting another
LLM response, and SHALL consume the park intent only after those persistence
obligations succeed

IF persistence fails or the runtime restarts while awaiting sub-agents
THE SYSTEM SHALL preserve or reconstruct the park intent from durable round and
registration identities so final fan-in retries parking without an LLM request

THE registration receipt SHALL identify the contract id, watched handle kind,
watched handle id, computed `expires_at`, and MAY include the registering
`tool_use_id` for audit

AFTER registration persistence commits and any post-commit cancellation compensation
confirms the contract remains registered and pending
THE SYSTEM SHALL emit exactly one best-effort `WakeContractRegistered` live SSE edge
from the persisted receipt data, visible only to authenticated conversation streams;
the durable full wake snapshot SHALL remain replay and source-of-truth state

**Rationale:** Registration is an acknowledgement that Phoenix accepted a durable
obligation to resume the conversation later. Observation happens after the
receipt exists, so restart recovery can reconstruct unfinished obligations.

---

### REQ-WAKE-002: Normalized Durable Contract Schema

THE SYSTEM SHALL persist wake contracts in normalized relational storage rather
than in JSON blobs

THE durable contract row SHALL expose queryable columns for:
- contract identity
- owning conversation identity
- handle kind
- handle id
- registering tool-use id
- registration timestamp
- expiry timestamp
- terminal status
- terminal cause
- resolution timestamp
- cancellation metadata needed for replay

THE durable schema SHALL NOT require `condition_json` or `fire_template_json`
columns

THE durable schema SHALL persist any captured bash or tmux tail output in child
rows keyed by `(contract_id, ordinal)`

THE durable schema SHALL make terminal-cause discriminators queryable without
`json_extract`-style inspection

**Rationale:** Wake contracts are operational state, not an opaque document.
Durable replay, operator queries, and startup reconciliation depend on
structurally queryable fields.

---

### REQ-WAKE-003: Bash-and-Tmux-Only Handle Scope

THE SYSTEM SHALL support wake registration only for these handle kinds:
- `Bash`
- `TmuxWindow`

THE SYSTEM SHALL reject registration for any other handle kind in this version

For `Bash`, the watched handle identity SHALL be the Phoenix bash handle id
returned by the asynchronous bash run surface

For `TmuxWindow`, the watched handle identity SHALL be the stable tmux-run window
id recorded in the tmux registry

**Rationale:** This version standardizes the runtime-observation protocol only
for handle kinds with clear terminal observation surfaces inside Phoenix.

---

### REQ-WAKE-004: Conversation Ownership and Continuation Transfer

A wake contract SHALL be owned by exactly one conversation id at a time

WHEN a continuation creates a successor conversation
THE SYSTEM SHALL transfer every pending wake contract owned by the predecessor to
the successor before any subsequent wake delivery

WHEN an unconsumed wake observation references a terminal contract owned by the
predecessor
THE SYSTEM SHALL transfer that contract's delivery ownership to the successor in the
same transaction as the observation, regardless of whether its cause is fired,
expired, forgotten, or cancelled

A consumed terminal contract SHALL remain historical ownership of the predecessor
unless another unconsumed observation still references it

THE SYSTEM SHALL preserve the same contract id, handle identity, receipt data,
and expiry deadline across that transfer

THE SYSTEM SHALL persist the normalized WorkScope that owns the watched handle and
SHALL use that registration lookup scope for every later handle observation

WHEN in-place Explore→Work approval transfers a live resource from a conversation
scope to a worktree scope
THE SYSTEM SHALL preflight every registry and evidence destination without mutation,
make both resource lookup keys resolve to the same resource, move tmux evidence,
and atomically persist conversation mode, cwd, and pending-contract scope migration
before retiring the old aliases and exposing the new scope through runtime context

Explore→Work scope migration SHALL affect pending contracts only; terminal contract
provenance SHALL remain unchanged. If any preflight, alias publication, evidence move,
or durable transaction step fails, THE SYSTEM SHALL restore the pre-approval mode,
cwd, context, old-only registry lookups, evidence, and contract scope so one retry can
perform the transfer. A destination collision SHALL be surfaced rather than clobbering
either resource.

A continuation transfer SHALL change only delivery ownership; it SHALL NOT change
the persisted registration WorkScope, including when a Direct continuation has a
different conversation-scoped WorkScope

WHEN a continuation creates a successor conversation
THE SYSTEM SHALL also transfer every unconsumed wake observation and every durable
but unaccepted wake resume request owned by the predecessor to the successor before
any subsequent wake delivery

A transferred durable wake resume request SHALL reference an exact copy of its
runtime-observation message in the successor's own message history, with a
successor-safe deterministic message identity; the predecessor's historical
message SHALL remain unchanged

The continuation transfer SHALL NOT create more than one successor observation or
more than one pending resume request for the same transferred snapshot

The continuation transfer rule SHALL apply regardless of whether the successor
inherits the predecessor's WorkScope

**Rationale:** Wake delivery ownership follows the conversation lineage, not the
resource-lifetime policy of the watched handle. Pending obligations and terminal contracts referenced by unconsumed observations
must move with those observations so no owed delivery remains bound to the
predecessor and predecessor deletion cannot cascade successor-owned delivery data.

---

### REQ-WAKE-005: Durable Inbox and Coalesced Resume

THE SYSTEM SHALL record terminal wake outcomes in a durable inbox before trying
to resume the owning conversation

Dispatcher admission SHALL select only conversations with an undelivered,
unconsumed observation whose `auto_resume` value is true, without materializing
terminal payloads or tails until dispatch proceeds for an eligible conversation

THE durable inbox SHALL guarantee that a pending terminal outcome survives
process restart until it is materialized into a durable runtime-observation message

THE SYSTEM SHALL durably record an unaccepted resume request in the same transaction
that materializes the bounded inbox snapshot and consumes those inbox rows

WHILE a conversation is busy
THE SYSTEM SHALL leave its wake inbox rows unconsumed and SHALL NOT materialize a
runtime-observation message or durable resume request

A durable resume request SHALL remain pending while dispatch cannot reach its idle
runtime and across process restart

WHEN multiple wake outcomes become pending for the same conversation while no
runtime is actively consuming that conversation
THE SYSTEM SHALL coalesce them into one resume request for that conversation
while preserving each outcome as a distinct inbox item

WHEN the runtime resumes a conversation because of wake inbox items
THE SYSTEM SHALL deliver all still-pending inbox items for that conversation as a
single continuation batch in deterministic order

THE SYSTEM SHALL permit new inbox items committed after a coalesced resume
request snapshot to remain pending for a later consumption batch

WHEN an idle runtime accepts a durable resume request
THE SYSTEM SHALL atomically persist the `LlmRequesting` state and mark that exact
resume request accepted before invoking the LLM

Duplicate or stale delivery of an already accepted resume request SHALL NOT create
another LLM turn

AN archived conversation SHALL NOT materialize or accept a wake resume

WHEN a conversation is archived after wake materialization but before acceptance
THE SYSTEM SHALL atomically suppress its pending resume request so it is neither
retryable nor able to invoke the LLM

**Rationale:** The wake plane's first duty is to durably remember that a
continuation is owed. Resume scheduling may be coalesced, but outcome payloads may
not be dropped or merged semantically.

---

### REQ-WAKE-006: Exactly-Once Resolution Transaction

WHEN Phoenix observes that a pending contract has reached a terminal outcome
THE SYSTEM SHALL resolve that contract in one atomic transaction that:
1. changes the contract from pending to terminal exactly once,
2. records the terminal cause and any normalized payload data,
3. appends one inbox item for later conversation delivery, and
4. records any wake-tail child rows associated with that outcome

THE SYSTEM SHALL make a second terminal transition for the same contract
unrepresentable

THE SYSTEM SHALL make it unrepresentable for an inbox item to exist without the
matching terminal contract row, or for a terminal contract row to require inbox
delivery but have no inbox item

**Rationale:** Wake resolution is a durable promise boundary. Contract state,
payload persistence, and owed-delivery bookkeeping must either all happen or none
of them happen.

---

### REQ-WAKE-007: Runtime Observation Semantics

THE SYSTEM SHALL run wake observation independently of conversation execution
state

FOR each pending bash or tmux contract, the observer SHALL determine one terminal
outcome according to the watched handle's runtime evidence or the contract's
expiry deadline

FOR bash handles, the observed runtime facts SHALL mirror the terminal statuses
and metadata exposed by the synchronous bash wait surface for an observed handle

FOR tmux handles, the observed runtime facts SHALL be derived from Phoenix's
durable tmux terminal evidence for the watched window, including exit information
when available and final captured tail output when available

THE observer SHALL prefer durable terminal evidence whose `evidence_at <=
expires_at` over expiry, even if Phoenix notices that evidence only after the
deadline

A contract SHALL expire when `now >= expires_at` and no qualifying evidence with
`evidence_at <= expires_at` exists

**Rationale:** The wake plane is a runtime observation service. Its semantics are
"what terminal fact became durably knowable before the deadline," not "what did
this poll tick happen to notice first."

---

### REQ-WAKE-008: Startup Reconciliation Before Serving

WHEN Phoenix starts
THE SYSTEM SHALL reconcile every non-terminal wake contract and retry every durable
unaccepted resume request before normal wake observation and periodic scheduling
begin

During startup reconciliation:
- contracts with durable terminal evidence whose `evidence_at <= expires_at`
  SHALL be resolved as fired,
- contracts whose deadline passed without such evidence but whose handle remains
  evaluable SHALL be resolved as expired,
- contracts whose handle can no longer be observed SHALL be resolved as forgotten,
  and
- contracts whose handle remains pending and observable SHALL be re-registered for
  normal runtime observation

THE SYSTEM SHALL apply the same inbox-writing and exactly-once resolution rules
during startup reconciliation as during live runtime observation

**Rationale:** Restart must not erase obligations or create a second, weaker wake
path. Startup reconciliation is the same durable protocol applied to preexisting
pending rows.

---

### REQ-WAKE-009: Terminal Outcome Vocabulary

Every accepted wake contract SHALL reach exactly one terminal cause:
- `Fired`
- `Expired`
- `Cancelled`
- `Forgotten`

`Fired` SHALL mean Phoenix observed the watched handle's terminal runtime outcome

`Expired` SHALL mean no qualifying terminal evidence existed before the delivery
deadline

`Cancelled` SHALL mean an explicit contract cancellation was requested

`Forgotten` SHALL mean Phoenix can no longer observe the watched handle well
enough to determine whether it would later have fired

THE persisted terminal cause SHALL be queryable independently of any payload body

**Rationale:** The wake plane owes one answer for every accepted contract, and the
kind of answer matters operationally.

---

### REQ-WAKE-010: Explicit Cancel Without Auto-Resume

WHEN a user or runtime surface explicitly cancels a pending wake contract
THE SYSTEM SHALL resolve that contract with terminal cause `Cancelled`

Contract cancellation SHALL cancel only the wake obligation, not the underlying
bash process or tmux window, unless another specification for that surface says
otherwise

THE SYSTEM SHALL append a durable wake observation for the cancelled contract

A cancelled wake contract SHALL NOT by itself schedule an LLM resume, create a
replacement wake contract, automatically restart observation, or automatically
resume waiting on the same handle

A later natural resume request MAY deliver the cancelled observation together
with any other still-pending wake observations

**Rationale:** Cancelling a wake contract means "stop owing me an immediate
continuation for this wait," not "reinterpret my intent and keep waiting another
way."

---

### REQ-WAKE-011: Lifecycle Blocking Is Separate From Busy Execution

THE SYSTEM SHALL derive a lifecycle-blocking signal for conversations with one or
more pending wake contracts

That lifecycle-blocking signal SHALL be distinct from the conversation runtime's
ordinary `is_busy` execution signal

A conversation with pending wake contracts MAY be runtime-idle while still being
lifecycle-blocked

Lifecycle actions that archive, abandon, mark merged, or hard-delete a
conversation SHALL reject or conflict while one or more wake contracts remain
pending

**Rationale:** A wake-pending conversation is not actively executing, but it is
also not lifecycle-free. Conflating these ideas in one busy bit loses precision.

---

### REQ-WAKE-012: Independent Multiple Contracts and Coalesced Scheduling

A conversation MAY own multiple pending wake contracts simultaneously

WHEN one contract resolves
THE SYSTEM SHALL NOT cancel sibling pending contracts automatically

WHEN several contracts for the same conversation resolve before that conversation
next resumes
THE SYSTEM SHALL preserve one inbox item per resolved contract and MAY coalesce
resume scheduling into a single continuation attempt

THE SYSTEM SHALL permit at most one active LLM execution for a conversation at a
time; wake-driven arrivals while that conversation is already executing SHALL
persist and wait for a later idle scheduling opportunity

**Rationale:** Wake contracts represent independent obligations. Scheduler
coalescing is an execution optimization, not a semantic merge.

---

### REQ-WAKE-013: Delivered Runtime Observation Payloads

WHEN a conversation runtime consumes wake inbox items
THE SYSTEM SHALL deliver typed Phoenix runtime observations, not synthetic tool
results

Each delivered observation SHALL be correlated by `contract_id`

Each delivered observation MAY include the original registering `tool_use_id` for
audit, but SHALL NOT rely on delayed tool-result attribution semantics

For `Bash`, the delivered observation SHALL include terminal status, exit
metadata when available, duration metadata when available, and the normalized
final tail window

For `TmuxWindow`, the delivered observation SHALL include terminal status, exit
metadata when available, and the normalized final captured tail window

**Rationale:** The agent should reason over the same terminal facts regardless of
whether it polled synchronously or used wake, but wake delivery is a distinct
runtime-observation surface rather than a delayed completion of the registration
call.

---

### REQ-WAKE-014: Ownership and Authorization

THE SYSTEM SHALL allow a conversation to register wake only on handles it owns
through its current ownership boundary

THE SYSTEM SHALL reject registration for handles outside that ownership boundary

THE SYSTEM SHALL reject cancellation requests from callers that do not own the
contract's current conversation

**Rationale:** Wake transfer can move ownership forward, but it never broadens who
may observe or cancel a contract.

---

### REQ-WAKE-015: Timeout Range, Default, and Cap

EVERY wake contract SHALL have a computed `expires_at` at registration time

THE SYSTEM SHALL apply a default wait duration of 600 seconds when the caller
omits one

THE SYSTEM SHALL accept explicit wait durations only in the inclusive range 1
through 1800 seconds

THE SYSTEM SHALL reject registration whose requested wait duration falls outside
that inclusive range

**Rationale:** Wake is a persisted commitment to spend future runtime work and
possible model budget. It must always be bounded.
