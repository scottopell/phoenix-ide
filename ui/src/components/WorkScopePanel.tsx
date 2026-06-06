/**
 * WorkScopePanel — right-adjacent dock surfacing the live runtime resources
 * a work scope owns: backgrounded bash handles, the tmux server, and the
 * browser session (specs/work-scope-ui/, REQ-WSUI-009 / REQ-WSUI-010).
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

import { useEffect, useState, useCallback } from 'react';
import { api } from '../api';
import type {
  WorkScopeInventory,
  BashHandleInventory,
  BashHandleState,
} from '../api';
import './WorkScopePanel.css';

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

function isLive(state: BashHandleState): boolean {
  return state === 'running' || state === 'kill_pending_kernel';
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

/** Count of resources that read as "running right now": live bash handles
 *  (running + kill_pending_kernel) plus a live (non-idle) browser session.
 *  Drives the collapsed-rail badge. */
function liveCount(inv: WorkScopeInventory | null): number {
  if (!inv) return 0;
  const liveBash = inv.bash.filter((h) => isLive(h.state)).length;
  const liveBrowser = inv.browser && inv.browser.state === 'live' ? 1 : 0;
  return liveBash + liveBrowser;
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
          {handle.ring_bytes_used != null && (
            <div className="ws-detail-line">
              <span className="ws-detail-key">output</span>
              <span className="ws-detail-val">{formatBytes(handle.ring_bytes_used)}</span>
            </div>
          )}
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

export function WorkScopePanel({ scopeKey, liveInventory, collapsed, onToggle, width }: Props) {
  const [fetched, setFetched] = useState<WorkScopeInventory | null>(null);
  const [error, setError] = useState(false);
  // Tick once a second so live elapsed times advance without an SSE push.
  const [now, setNow] = useState(() => Date.now());

  const loadInitial = useCallback(async () => {
    setError(false);
    try {
      const inv = await api.getWorkScopeInventory(scopeKey);
      setFetched(inv);
    } catch {
      setError(true);
    }
  }, [scopeKey]);

  // Initial pull (REQ-WSUI-006). Re-runs when the scope key changes.
  useEffect(() => {
    setFetched(null);
    void loadInitial();
  }, [loadInitial]);

  useEffect(() => {
    if (collapsed) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [collapsed]);

  // Live SSE-fed inventory wins; otherwise fall back to the initial fetch.
  const inventory = liveInventory ?? fetched;
  const count = liveCount(inventory);
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
      <div className="ws-body">
        {error && !inventory && (
          <div className="ws-empty ws-empty--error">
            inventory failed to load
            <button className="ws-retry" onClick={() => void loadInitial()}>
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
    </aside>
  );
}
