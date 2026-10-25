import { useState } from 'react';
import type { FormEvent } from 'react';
import { goalSource, goalStateLabel, goalStateReason } from '../goalApi';
import type { GoalDisposition, GoalEntry } from '../goalApi';
import type { useConversationGoals } from '../hooks/useConversationGoals';
import './ConversationGoals.css';

type Model = ReturnType<typeof useConversationGoals>;

export function ConversationGoalSummary({ model }: { model: Model }) {
  const facts = model.facts;
  return <span className="conversation-goal-summary">GOAL {facts.kind === 'loading' ? '…'
    : facts.kind === 'unavailable' ? '— unavailable'
      : facts.kind === 'error' ? '✗ read failed'
        : facts.current ? `${goalStateLabel(facts.current)} · v${facts.current.snapshot.version}${facts.control?.stopped ? ' · STOP' : ''}` : `none${facts.control?.stopped ? ' · STOP' : ''}`}</span>;
}

function GoalFact({ entry }: { entry: GoalEntry }) {
  const requested = goalSource(entry);
  const applied = goalSource(entry, 'applied_by');
  return <div className="conversation-goal-fact">
    <strong>{entry.snapshot.objective}</strong>
    <div>{goalStateLabel(entry)} · v{entry.snapshot.version} · {entry.snapshot.goal_id}</div>
    {goalStateReason(entry) ? <div>{goalStateReason(entry)}</div> : null}
    <div className="conversation-goal-source">{requested}{applied !== requested ? ` · Applied by ${applied}` : ''}</div>
  </div>;
}

export function ConversationGoalDetails({ model }: { model: Model }) {
  const [objective, setObjective] = useState('');
  const [state, setState] = useState<'underway' | 'blocked' | 'complete' | 'exhausted'>('underway');
  const [reason, setReason] = useState('');
  const facts = model.facts;
  const current = facts.kind === 'ready' ? facts.current : null;
  const terminal = current && (goalStateLabel(current) === 'Complete' || goalStateLabel(current) === 'Exhausted' || goalStateLabel(current) === 'Suspended');
  const create = (event: FormEvent) => {
    event.preventDefault();
    if (!model.canWrite || !objective.trim() || facts.kind !== 'ready') return;
    void model.submit({ kind: 'create', body: { request_id: crypto.randomUUID(), expected_version: current?.snapshot.version ?? 0, objective } });
  };
  const change = (event: FormEvent) => {
    event.preventDefault();
    if (!model.canWrite || !current || terminal || (state !== 'underway' && !reason.trim())) return;
    const disposition: GoalDisposition = state === 'underway' ? { state } : { state, reason };
    void model.submit({ kind: 'state', body: { request_id: crypto.randomUUID(), expected_version: current.snapshot.version, goal_id: current.snapshot.goal_id, disposition } });
  };
  return <section className="conversation-goals" aria-label="Conversation goal">
    <div className="conversation-goal-actions"><strong>Goal facts</strong><button type="button" onClick={() => { void model.refresh(); }} disabled={model.unresolved?.outcome === 'sending'}>Refresh goals</button></div>
    {facts.kind === 'loading' ? <p role="status">Reading goal facts…</p>
      : facts.kind === 'unavailable' || facts.kind === 'error' ? <p role="status">{facts.message}</p>
        : <>
          {current ? <GoalFact entry={current} /> : <p>No recorded goal.</p>}
          <p>Control {facts.control ? `v${facts.control.version} · ${facts.control.stopped ? 'Stopped' : 'Stop latch released'}` : 'not initialized (CAS 0)'} · Reading facts grants no execution permission.</p>
          {!model.writable ? <p>Read-only: History or Close admission. Commands unavailable.</p> : null}
          {model.writable ? <fieldset disabled={!model.canWrite}>
            <form onSubmit={create}>
              <label>New objective<textarea value={objective} onChange={(event) => setObjective(event.target.value)} /></label>
              <button type="submit" disabled={!objective.trim()}>{current ? 'Replace goal' : 'Create goal'}</button>
            </form>
            {current ? <form onSubmit={change}>
              <label>Explicit goal state<select value={state} onChange={(event) => setState(event.target.value as typeof state)}>
                <option value="underway">Underway</option><option value="blocked">Blocked</option><option value="complete">Complete</option><option value="exhausted">Exhausted</option>
              </select></label>
              {state !== 'underway' ? <label>State reason<textarea value={reason} onChange={(event) => setReason(event.target.value)} /></label> : null}
              <button type="submit" disabled={Boolean(terminal) || (state !== 'underway' && !reason.trim())}>Record state</button>
            </form> : null}
            <button type="button" disabled={Boolean(facts.control?.stopped)} onClick={() => {
              void model.submit({ kind: 'stop', body: { request_id: crypto.randomUUID(), command: 'stop', expected_control_version: facts.control?.version ?? 0 } });
            }}>Stop goal work</button>
          </fieldset> : null}
          <details><summary>Recorded goal history ({facts.history.length})</summary>
            <ol>{facts.history.map((entry) => <li key={entry.snapshot.version}><GoalFact entry={entry} /></li>)}</ol>
            {facts.more ? <button type="button" onClick={() => { void model.moreHistory(); }} disabled={model.busyHistory}>{model.busyHistory ? 'Reading history…' : 'More goal history'}</button> : null}
          </details>
        </>}
    {model.notice ? <p role="status">{model.notice}</p> : null}
    {model.unresolved ? <div role="status">{model.unresolved.outcome === 'sending' ? 'Sending command…' : 'Outcome unknown; new commands are blocked.'}
      {model.unresolved.outcome === 'unknown' ? <button type="button" disabled={!model.writable} onClick={() => { if (model.unresolved) void model.submit(model.unresolved.command, true); }}>Retry exact command</button> : null}
    </div> : null}
  </section>;
}
