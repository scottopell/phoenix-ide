/**
 * Work-scope observability surface for the live runtime resources a work
 * scope owns: backgrounded bash handles, the tmux server, and the browser
 * session (specs/work-scope-ui/, REQ-WSUI-009 / REQ-WSUI-010).
 *
 * Two surfaces share the row/badge rendering here:
 *   - `WorkScopeSection` — a collapsible section in the conversation page's
 *     left `FileExplorerPanel`, stacked with Files/Skills/Tasks (REQ-WSUI-010).
 *   - `WorkScopePanel` — the standalone right-adjacent dock on the chain page,
 *     which has no left explorer panel to host a section (REQ-WSUI-009).
 *
 * Data sources:
 *   - Conversation page: the live `liveInventory` prop (the conversation
 *     atom's `workScope`, kept fresh by the `work_scope_update` SSE push) is
 *     authoritative when present. An initial fetch fills the gap before the
 *     first push lands.
 *   - Chain page: no per-conversation SSE channel, so the initial fetch is
 *     the only data source (REQ-WSUI-009 rationale). One query against the
 *     chain root's scope key is complete — resources are WorkScope-keyed, so
 *     there is no per-member aggregation.
 *
 * Density (AGENTS.md UI Design Philosophy): collapsed, the rail is a single
 * live-count badge ("is anything running?"); expanded, each resource is one
 * dense row with an inline status glyph + elapsed time ("what, and for how
 * long?"); the bash ring tail / pid detail is one disclosure deeper.
 */

import { useEffect, useState, useCallback, useRef } from 'react';
import { api } from '../api';
import type {
  WorkScopeInventory,
  BashHandleInventory,
  BashHandleState,
} from '../api';
import { isLive, hasRunningBash, workScopeLiveCount } from './workScopeHelpers';
import './WorkScopePanel.css';

/** Cadence of the running-handle inventory poll. `output_bytes` grows
 *  continuously as a process emits output, but the `work_scope_update` SSE push
 *  is edge-triggered on bash state transitions only — between transitions the
 *  byte count would otherwise stay frozen. */
const RUNNING_POLL_INTERVAL_MS = 2000;

/** A `live` browser session whose last activity is older than this reads as
 *  "idle" — a purely client-side presentation over `idle_ms`, distinct from
 *  the wire `state` which is only `live` | `torn_down` (REQ-WSUI-004 /
 *  REQ-WSUI-010). */
const BROWSER_IDLE_THRESHOLD_MS = 60_000;

interface Props {
  /** The scope key to query (`work_scope_key` on the conversation, or the
   *  chain root's scope key). */
  scopeKey: string;
  /** Live inventory from the conversation atom (SSE-fed). When provided it
   *  overrides the panel's initial fetch — it is at least as fresh. Omit on
   *  the chain page, which has no per-conversation push channel. */
  liveInventory?: WorkScopeInventory | null;
  collapsed: boolean;
  onToggle: () => void;
  /** Width in px when expanded — driven by useResizablePane. */
  width?: number | undefined;
}

/** Inline status glyph + class per bash handle state, following the
 *  Valid/Will-create/Invalid/Loading feedback conventions (AGENTS.md). */
function bashGlyph(state: BashHandleState): { glyph: string; cls: string; title: string } {
  switch (state) {
    case 'running':
      return { glyph: '✓', cls: 'ws-glyph--ok', title: 'running' };
    case 'kill_pending_kernel':
      return { glyph: '⏱', cls: 'ws-glyph--warn', title: 'kill pending (kernel)' };
    case 'tombstoned':
      return { glyph: '○', cls: 'ws-glyph--muted', title: 'tombstoned' };
  }
}

/** Human-rounded elapsed string for a started/finished handle. For a live
 *  handle we show time since `started_at`; for a terminal handle we show its
 *  recorded `duration_ms`. */
function elapsedLabel(handle: BashHandleInventory, now: number): string {
  const ms =
    handle.duration_ms != null
      ? handle.duration_ms
      : Math.max(0, now - Date.parse(handle.started_at));
  return formatDuration(ms);
}

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function BashRow({ handle, now }: { handle: BashHandleInventory; now: number }) {
  const [open, setOpen] = useState(false);
  const { glyph, cls, title } = bashGlyph(handle.state);
  const live = isLive(handle.state);
  const label = handle.label || handle.cmd;
  return (
    <div className={`ws-row ws-row--bash${live ? '' : ' ws-row--dead'}`}>
      <button
        className="ws-row-main"
        onClick={() => setOpen((o) => !o)}
        title="Toggle details"
      >
        <span className={`ws-glyph ${cls}`} title={title} aria-label={title}>
          {glyph}
        </span>
        <span className="ws-row-label" title={handle.cmd}>
          {label}
        </span>
        <span className="ws-row-meta">{elapsedLabel(handle, now)}</span>
      </button>
      {open && (
        <div className="ws-row-detail">
          <div className="ws-detail-line">
            <span className="ws-detail-key">id</span>
            <span className="ws-detail-val">{handle.handle_id}</span>
          </div>
          <div className="ws-detail-line">
            <span className="ws-detail-key">cmd</span>
            <span className="ws-detail-val ws-detail-val--cmd">{handle.cmd}</span>
          </div>
          {handle.pid != null && (
            <div className="ws-detail-line">
              <span className="ws-detail-key">pid</span>
              <span className="ws-detail-val">
                {handle.pid}
                {handle.pgid != null ? ` (pgid ${handle.pgid})` : ''}
              </span>
            </div>
          )}
          <div className="ws-detail-line">
            <span className="ws-detail-key">output</span>
            <span className="ws-detail-val">{formatBytes(handle.output_bytes)}</span>
          </div>
        </div>
      )}
    </div>
  );
}

function TmuxRow({ status }: { status: 'not_probed' | 'live' | 'gone' }) {
  const map = {
    live: { glyph: '✓', cls: 'ws-glyph--ok', text: 'live' },
    gone: { glyph: '✗', cls: 'ws-glyph--err', text: 'gone' },
    not_probed: { glyph: '—', cls: 'ws-glyph--muted', text: 'not probed' },
  } as const;
  const { glyph, cls, text } = map[status];
  return (
    <div className="ws-row">
      <div className="ws-row-main ws-row-main--static">
        <span className={`ws-glyph ${cls}`} title={text} aria-label={text}>
          {glyph}
        </span>
        <span className="ws-row-label">tmux server</span>
        <span className="ws-row-meta">{text}</span>
      </div>
    </div>
  );
}

function BrowserRow({ state, idleMs }: { state: 'live' | 'torn_down'; idleMs: number }) {
  // "idle" is a client-side display over idle_ms; the wire state stays live.
  const idle = state === 'live' && idleMs >= BROWSER_IDLE_THRESHOLD_MS;
  const display =
    state === 'torn_down'
      ? { glyph: '○', cls: 'ws-glyph--muted', text: 'torn down' }
      : idle
        ? { glyph: '○', cls: 'ws-glyph--warn', text: `idle ${formatDuration(idleMs)}` }
        : { glyph: '✓', cls: 'ws-glyph--ok', text: 'live' };
  return (
    <div className={`ws-row${state === 'torn_down' ? ' ws-row--dead' : ''}`}>
      <div className="ws-row-main ws-row-main--static">
        <span className={`ws-glyph ${display.cls}`} title={display.text} aria-label={display.text}>
          {display.glyph}
        </span>
        <span className="ws-row-label">browser</span>
        <span className="ws-row-meta">{display.text}</span>
      </div>
    </div>
  );
}

/**
 * Resolves the inventory to render via last-arrival-wins over a single local
 * snapshot fed by three sources, all of which are FULL inventory snapshots of
 * the same scope key (REQ-WSUI-006):
 *
 *   1. the initial fetch keyed by `scopeKey`,
 *   2. the SSE-fed `liveInventory` prop (the conversation atom's `workScope`,
 *      pushed on bash state transitions), and
 *   3. a poll while any bash handle is running.
 *
 * The SSE push is edge-triggered on state transitions, so `output_bytes`
 * (which grows continuously as a process emits output) stays frozen between
 * transitions. The poll closes that gap: while at least one bash handle is
 * `running` / `kill_pending_kernel` and the surface is active, it re-fetches
 * every {@link RUNNING_POLL_INTERVAL_MS} so byte counts (and any other live
 * fields) advance. It stops once nothing is running or the surface unmounts —
 * self-limiting, no unbounded timers.
 *
 * Each source writes the same local `displayed` state; the most recent write
 * wins. This keeps the single-writer atom contract — none of these paths touch
 * the atom's `workScope`, which stays written only by the SSE reducer.
 *
 * `active` gates both the per-second elapsed-time tick and the running poll:
 * callers pass `false` while the surface is collapsed so an off-screen panel
 * does no background work.
 */
function useWorkScopeInventory(
  scopeKey: string,
  liveInventory: WorkScopeInventory | null | undefined,
  active: boolean,
) {
  const [displayed, setDisplayed] = useState<WorkScopeInventory | null>(null);
  const [error, setError] = useState(false);
  // Tick once a second so live elapsed times advance without an SSE push.
  const [now, setNow] = useState(() => Date.now());

  // Latest scope key for the poll callback without re-creating it per render.
  const scopeKeyRef = useRef(scopeKey);
  scopeKeyRef.current = scopeKey;

  const fetchSnapshot = useCallback(async () => {
    setError(false);
    try {
      const inv = await api.getWorkScopeInventory(scopeKeyRef.current);
      setDisplayed(inv);
    } catch {
      setError(true);
    }
  }, []);

  // Initial pull (REQ-WSUI-006). Re-runs (and clears the stale snapshot) when
  // the scope key changes.
  useEffect(() => {
    setDisplayed(null);
    void fetchSnapshot();
  }, [scopeKey, fetchSnapshot]);

  // SSE push: a fresh full snapshot for this scope. Last-arrival-wins.
  useEffect(() => {
    if (liveInventory != null) setDisplayed(liveInventory);
  }, [liveInventory]);

  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);

  // Running-handle poll: only while a bash handle is actually running and the
  // surface is active. Gated on `displayed` so it stops once everything is
  // tombstoned and starts as soon as a handle spawns.
  const pollRunning = active && hasRunningBash(displayed);
  useEffect(() => {
    if (!pollRunning) return;
    const id = setInterval(() => void fetchSnapshot(), RUNNING_POLL_INTERVAL_MS);
    return () => clearInterval(id);
  }, [pollRunning, fetchSnapshot]);

  return { inventory: displayed, error, now, retry: fetchSnapshot };
}

/**
 * The dense resource body shared by both surfaces: bash handles, tmux, and
 * browser sections. Pure presentation over a resolved inventory.
 */
function WorkScopeBody({
  inventory,
  error,
  now,
  onRetry,
}: {
  inventory: WorkScopeInventory | null;
  error: boolean;
  now: number;
  onRetry: () => void;
}) {
  return (
    <div className="ws-body">
      {error && !inventory && (
        <div className="ws-empty ws-empty--error">
          inventory failed to load
          <button className="ws-retry" onClick={onRetry}>
            retry
          </button>
        </div>
      )}
      {!error && !inventory && <div className="ws-empty">loading&hellip;</div>}
      {inventory && (
        <>
          <section className="ws-section">
            <div className="ws-section-head">bash ({inventory.bash.length})</div>
            {inventory.bash.length === 0 ? (
              <div className="ws-empty">no handles</div>
            ) : (
              inventory.bash.map((h) => <BashRow key={h.handle_id} handle={h} now={now} />)
            )}
          </section>
          <section className="ws-section">
            <div className="ws-section-head">tmux</div>
            {inventory.tmux ? (
              <TmuxRow status={inventory.tmux.status} />
            ) : (
              <div className="ws-empty">no server</div>
            )}
          </section>
          <section className="ws-section">
            <div className="ws-section-head">browser</div>
            {inventory.browser ? (
              <BrowserRow state={inventory.browser.state} idleMs={inventory.browser.idle_ms} />
            ) : (
              <div className="ws-empty">no session</div>
            )}
          </section>
        </>
      )}
    </div>
  );
}

interface SectionProps {
  /** The scope key to query (`work_scope_key` on the conversation). */
  scopeKey: string;
  /** Live inventory from the conversation atom (SSE-fed). When provided it
   *  overrides the section's initial fetch — it is at least as fresh. */
  liveInventory?: WorkScopeInventory | null | undefined;
  expanded: boolean;
  onToggleExpanded: (expanded: boolean) => void;
}

/**
 * Work-scope as a collapsible section inside the conversation page's left
 * `FileExplorerPanel`, stacked with Files/Skills/Tasks (REQ-WSUI-010). Mirrors
 * the header + own-expand-state pattern of `SkillsPanel` / `TasksPanel`; the
 * dense resource body is the shared `WorkScopeBody`.
 */
export function WorkScopeSection({ scopeKey, liveInventory, expanded, onToggleExpanded }: SectionProps) {
  const { inventory, error, now, retry } = useWorkScopeInventory(scopeKey, liveInventory, expanded);
  const count = workScopeLiveCount(inventory);

  return (
    <div className={`ws-section-panel${expanded ? ' is-expanded' : ''}`}>
      <button className="ws-section-panel-header" onClick={() => onToggleExpanded(!expanded)}>
        <span className={`ws-section-panel-chevron${expanded ? ' expanded' : ''}`}>&#9654;</span>
        <span className="ws-section-panel-summary">Work scope</span>
        <span className={`ws-count-badge${count > 0 ? ' ws-count-badge--active' : ''}`}>{count}</span>
      </button>
      {expanded && (
        <WorkScopeBody inventory={inventory} error={error} now={now} onRetry={() => void retry()} />
      )}
    </div>
  );
}

/**
 * Standalone right-adjacent dock for the chain page (REQ-WSUI-009), which has
 * no left explorer panel to host a section. Collapsed it is a single
 * live-count badge rail; expanded it shows the shared resource body.
 */
export function WorkScopePanel({ scopeKey, liveInventory, collapsed, onToggle, width }: Props) {
  const { inventory, error, now, retry } = useWorkScopeInventory(scopeKey, liveInventory, !collapsed);
  const count = workScopeLiveCount(inventory);
  const browserGlyph = inventory?.browser?.state === 'live' ? '◉' : null;
  const tmuxLive = inventory?.tmux?.status === 'live';

  if (collapsed) {
    return (
      <aside className="ws-panel ws-panel--collapsed">
        <button className="ws-toggle" onClick={onToggle} title="Expand work scope">
          &#9664;
        </button>
        <div className="ws-collapsed-stack">
          <button
            className={`ws-count-badge${count > 0 ? ' ws-count-badge--active' : ''}`}
            onClick={onToggle}
            title={`${count} running resource${count === 1 ? '' : 's'}`}
          >
            {count}
          </button>
          {browserGlyph && (
            <span className="ws-collapsed-ind" title="browser session live">
              {browserGlyph}
            </span>
          )}
          {tmuxLive && (
            <span className="ws-collapsed-ind" title="tmux server live">
              T
            </span>
          )}
        </div>
      </aside>
    );
  }

  return (
    <aside
      className="ws-panel ws-panel--expanded"
      style={width !== undefined ? { width: `${width}px`, minWidth: `${width}px` } : undefined}
    >
      <div className="ws-header">
        <button className="ws-toggle" onClick={onToggle} title="Collapse">
          &#9656;
        </button>
        <span className="ws-title">Work scope</span>
        <span className={`ws-count-badge${count > 0 ? ' ws-count-badge--active' : ''}`}>{count}</span>
      </div>
      <WorkScopeBody inventory={inventory} error={error} now={now} onRetry={() => void retry()} />
    </aside>
  );
}
