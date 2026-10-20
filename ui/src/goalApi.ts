import * as v from 'valibot';

const text = v.pipe(v.string(), v.check((value) => value.trim().length > 0, 'Expected nonblank text.'));
const counter = v.pipe(v.number(), v.safeInteger(), v.minValue(0));
const positive = v.pipe(counter, v.minValue(1));
const uuid = v.pipe(v.string(), v.uuid());
const state = v.union([
  v.literal('Underway'),
  v.strictObject({ Blocked: text }),
  v.strictObject({ Complete: text }),
  v.strictObject({ Suspended: text }),
  v.strictObject({ Exhausted: text }),
]);
const execution = v.strictObject({ workflow_id: positive, turn_id: positive });
const agentSource = v.strictObject({ product_conversation_id: text, transcript_id: text, execution, message_id: text, tool_use_id: text });
const actor = v.union([
  v.strictObject({ UserControl: v.strictObject({ principal_id: uuid, request_id: uuid, command: v.picklist(['create', 'state']) }) }),
  v.strictObject({ UserApi: v.strictObject({ transcript_id: text, message_id: text }) }),
  v.strictObject({ OwningAgent: agentSource }),
  v.strictObject({ Coordinator: agentSource }),
  v.strictObject({ GeneratedHost: v.strictObject({ action_id: text }) }),
]);
const observation = v.union([
  v.strictObject({ NoGoal: v.strictObject({ observed_version: counter }) }),
  v.strictObject({ Goal: v.strictObject({ owner: text, goal_id: text, version: positive }) }),
]);
const mutation = v.strictObject({
  owner: text, expected_version: counter, action_id: text,
  attribution: v.strictObject({ requested_by: actor, applied_by: actor }),
  change: v.union([
    v.strictObject({ Create: v.strictObject({ goal_id: text, objective: text }) }),
    v.strictObject({ SetDisposition: v.strictObject({ goal_id: text, disposition: state }) }),
    v.strictObject({ Report: v.strictObject({ binding: v.strictObject({ owner: text, execution, observation }), disposition: state }) }),
  ]),
});
export const goalEntrySchema = v.strictObject({
  snapshot: v.strictObject({ owner: text, goal_id: text, objective: text, state, version: positive }),
  mutation, recorded_at_us: counter,
});
export const controlSchema = v.strictObject({ version: positive, stopped: v.boolean() });
const userReceipt = v.strictObject({ request_id: uuid, result: v.variant('outcome', [
  v.strictObject({ outcome: v.literal('accepted'), entry: goalEntrySchema }),
  v.strictObject({ outcome: v.literal('rejected'), reason: v.picklist(['version', 'busy', 'transition']), observed_version: counter, recorded_at: counter }),
]) });
const controlReceipt = v.strictObject({
  request_id: uuid, owner: text, command: v.picklist(['stop', 'release']),
  principal_id: uuid, expected_control_version: counter, recorded_at: counter,
  result: v.variant('outcome', [
    v.strictObject({ outcome: v.literal('accepted'), snapshot: controlSchema }),
    v.strictObject({ outcome: v.literal('rejected_version'), observed: v.nullable(controlSchema) }),
  ]),
});
export type GoalEntry = v.InferOutput<typeof goalEntrySchema>;
export type GoalControl = v.InferOutput<typeof controlSchema>;
export type GoalDisposition =
  | { state: 'underway' }
  | { state: 'blocked'; reason: string }
  | { state: 'complete'; reason: string }
  | { state: 'exhausted'; reason: string };
export type GoalCommand =
  | { kind: 'create'; body: { request_id: string; expected_version: number; objective: string } }
  | { kind: 'state'; body: { request_id: string; expected_version: number; goal_id: string; disposition: GoalDisposition } }
  | { kind: 'stop'; body: { request_id: string; command: 'stop'; expected_control_version: number } };

export class GoalApiError extends Error {
  constructor(message: string, public readonly status: number | null, public readonly outcome: 'known' | 'unknown') {
    super(message);
  }
}
const root = (owner: string) => `/api/product-conversations/${encodeURIComponent(owner)}`;
const message = (status: number) => status === 503 ? 'Goals unavailable — services are not installed.'
  : status === 401 || status === 403 ? 'Owner authentication required.'
    : status === 409 ? 'Command refused by admission or request fencing. Refresh facts.'
      : `Goal request failed (${status}).`;

const uninstalledError = v.strictObject({ error: v.picklist(['goal services are not installed', 'goal control is not installed']) });
const admissionError = v.strictObject({ error: v.picklist([
  'goal command admission changed or is fenced',
  'request identity has different input',
  'control is stopped or uninitialized',
]) });
async function isUninstalled(response: Response): Promise<boolean> {
  try { return v.is(uninstalledError, await response.json()); } catch { return false; }
}
function matchesDisposition(actual: GoalEntry['snapshot']['state'], requested: GoalDisposition): boolean {
  switch (requested.state) {
    case 'underway': return actual === 'Underway';
    case 'blocked': return typeof actual === 'object' && 'Blocked' in actual && actual.Blocked === requested.reason;
    case 'complete': return typeof actual === 'object' && 'Complete' in actual && actual.Complete === requested.reason;
    case 'exhausted': return typeof actual === 'object' && 'Exhausted' in actual && actual.Exhausted === requested.reason;
  }
}

async function read<T>(url: string, schema: v.GenericSchema<unknown, T>, signal?: AbortSignal): Promise<T> {
  const response = await fetch(url, signal ? { signal } : undefined);
  if (!response.ok) {
    const detail = response.status === 503 && !await isUninstalled(response)
      ? 'Goal services unavailable (503); no facts read.' : message(response.status);
    throw new GoalApiError(detail, response.status, 'known');
  }
  return v.parse(schema, await response.json());
}
function checkOwner(entry: GoalEntry, owner: string) {
  if (entry.snapshot.owner !== owner || entry.mutation.owner !== owner) throw new Error('Goal response owner mismatch.');
}
export const goalApi = {
  async current(owner: string, signal?: AbortSignal) {
    const entry = await read(`${root(owner)}/goal`, v.nullable(goalEntrySchema), signal);
    if (entry) checkOwner(entry, owner);
    return entry;
  },
  control(owner: string, signal?: AbortSignal) {
    return read(`${root(owner)}/goal-control`, v.nullable(controlSchema), signal);
  },
  async history(owner: string, after: number, signal?: AbortSignal) {
    v.parse(counter, after);
    const entries = await read(`${root(owner)}/goal/history?after=${after}`, v.pipe(v.array(goalEntrySchema), v.maxLength(100)), signal);
    let previous = after;
    for (const entry of entries) {
      checkOwner(entry, owner);
      if (entry.snapshot.version <= previous) throw new Error('Unordered goal history.');
      previous = entry.snapshot.version;
    }
    return entries;
  },
  async command(owner: string, command: GoalCommand): Promise<string> {
    let response: Response;
    try {
      response = await fetch(`${root(owner)}/${command.kind === 'stop' ? 'goal-control' : command.kind === 'state' ? 'goal/state' : 'goal'}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(command.body),
      });
    } catch {
      throw new GoalApiError('Command outcome unknown. Retry this exact request.', null, 'unknown');
    }
    if (response.status === 503 && await isUninstalled(response)) {
      throw new GoalApiError(message(503), 503, 'known');
    }
    if (response.status !== 200 && response.status !== 409) {
      const known = [400, 401, 403, 404, 405, 413, 415, 422, 429].includes(response.status);
      throw new GoalApiError(known ? message(response.status) : 'Command outcome unknown. Retry this exact request.',
        response.status, known ? 'known' : 'unknown');
    }
    try {
      const body: unknown = await response.json();
      if (response.status === 409 && v.is(admissionError, body)) {
        throw new GoalApiError(message(409), 409, 'known');
      }
      if (command.kind === 'stop') {
        const receipt = v.parse(controlReceipt, body);
        if (receipt.request_id !== command.body.request_id || receipt.owner !== owner || receipt.command !== 'stop' || receipt.expected_control_version !== command.body.expected_control_version) throw new Error('Receipt mismatch');
        if ((receipt.result.outcome === 'accepted') !== (response.status === 200)) throw new Error('Status mismatch');
        if (receipt.result.outcome === 'accepted' && (!receipt.result.snapshot.stopped
          || receipt.result.snapshot.version !== command.body.expected_control_version + 1)) throw new Error('Stop receipt mismatch');
        return receipt.result.outcome === 'accepted' ? 'Stop recorded. No execution permission granted.' : 'Stop refused: control version changed.';
      }
      const receipt = v.parse(userReceipt, body);
      if (receipt.request_id !== command.body.request_id) throw new Error('Receipt mismatch');
      if ((receipt.result.outcome === 'accepted') !== (response.status === 200)) throw new Error('Status mismatch');
      if (receipt.result.outcome === 'accepted') {
        const entry = receipt.result.entry;
        checkOwner(entry, owner);
        const { requested_by: actor, applied_by: applier } = entry.mutation.attribution;
        if (entry.snapshot.version !== command.body.expected_version + 1
          || entry.mutation.expected_version !== command.body.expected_version
          || !('UserControl' in actor) || actor.UserControl.request_id !== command.body.request_id
          || actor.UserControl.command !== command.kind
          || !('UserControl' in applier) || applier.UserControl.request_id !== command.body.request_id
          || applier.UserControl.command !== command.kind
          || applier.UserControl.principal_id !== actor.UserControl.principal_id) throw new Error('Command receipt mismatch');
        if (command.kind === 'create') {
          if (!('Create' in entry.mutation.change)
            || entry.mutation.change.Create.objective !== command.body.objective
            || entry.snapshot.objective !== command.body.objective
            || entry.snapshot.goal_id !== entry.mutation.change.Create.goal_id
            || entry.snapshot.state !== 'Underway') throw new Error('Create receipt mismatch');
        } else if (!('SetDisposition' in entry.mutation.change)
          || entry.mutation.change.SetDisposition.goal_id !== command.body.goal_id
          || entry.snapshot.goal_id !== command.body.goal_id
          || !matchesDisposition(entry.mutation.change.SetDisposition.disposition, command.body.disposition)
          || !matchesDisposition(entry.snapshot.state, command.body.disposition)) throw new Error('State receipt mismatch');
        return 'Command recorded; no execution permission granted.';
      }
      return `Command refused: ${receipt.result.reason}. Refresh facts before a new command.`;
    } catch (error) {
      if (error instanceof GoalApiError) throw error;
      throw new GoalApiError('Invalid receipt: command outcome unknown. Retry this exact request.', response.status, 'unknown');
    }
  },
};

export function goalStateLabel(entry: GoalEntry): string {
  const state = entry.snapshot.state;
  if (state === 'Underway') return 'Underway';
  if ('Blocked' in state) return 'Blocked';
  if ('Suspended' in state) return 'Suspended';
  if ('Complete' in state) return 'Complete';
  return 'Exhausted';
}
export function goalStateReason(entry: GoalEntry): string | null {
  const state = entry.snapshot.state;
  if (state === 'Underway') return null;
  if ('Blocked' in state) return state.Blocked;
  if ('Suspended' in state) return state.Suspended;
  if ('Complete' in state) return state.Complete;
  return state.Exhausted;
}
export function goalSource(entry: GoalEntry, role: 'requested_by' | 'applied_by' = 'requested_by'): string {
  const actor = entry.mutation.attribution[role];
  if ('UserControl' in actor) return `Owner ${actor.UserControl.principal_id} · ${actor.UserControl.command} · request ${actor.UserControl.request_id}`;
  if ('UserApi' in actor) return `User API · transcript ${actor.UserApi.transcript_id} · message ${actor.UserApi.message_id}`;
  if ('GeneratedHost' in actor) return `Host action ${actor.GeneratedHost.action_id}`;
  const source = 'OwningAgent' in actor ? actor.OwningAgent : actor.Coordinator;
  return `${'OwningAgent' in actor ? 'Agent' : 'Coordinator'} ${source.product_conversation_id} · transcript ${source.transcript_id} · workflow ${source.execution.workflow_id} · turn ${source.execution.turn_id} · message ${source.message_id} · tool ${source.tool_use_id}`;
}
