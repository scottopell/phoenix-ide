/**
 * ProcessInspectorPanel — the per-handle live drill-down rendered in the
 * meta-viewer slot's `inspect` kind (specs/process-inspector/, REQ-PINSP-007 /
 * REQ-PINSP-008).
 *
 * Three sections, information-dense (AGENTS.md UI Design Philosophy):
 *   - an identity/state header (cmd, label, state glyph, pid/pgid while live,
 *     exit cause when terminal, started/elapsed);
 *   - a live OUTPUT pane (monospace, line-buffered, autoscroll-with-pause,
 *     truncation marker on `truncated_before`); and
 *   - a compact RESOURCE readout (cpu %, proportional memory, process count),
 *     rendering `—` for a null metric (capability gap) rather than a misleading 0.
 *
 * Transport (REQ-PINSP-005 / REQ-PINSP-006): a polling pull view. On open it
 * seeds with a no-`since` fetch (a recent tail); each subsequent ~1s poll passes
 * `since = last end_offset` and APPENDS the returned lines, advancing the tracked
 * offset. Polling stops when the handle is terminal (`tombstoned`) — the final
 * snapshot is rendered and nothing more can change — or on unmount. There is no
 * push transport: the output is offset-shaped and the resource sample only makes
 * sense while a viewer is open.
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api, NotFoundError } from '../api';
import type { BashHandleInspection, BashHandleState } from '../api';
import './ProcessInspectorPanel.css';

/** Polling cadence while open on a live handle (REQ-PINSP-006). */
const POLL_INTERVAL_MS = 1000;

/** A rendered output entry: either a real ring line or a synthetic gap marker
 *  inserted when a poll reports `truncated_before` (output evicted between
 *  polls, REQ-PINSP-008). The marker is structurally distinct from a line so it
 *  can never be confused for process output. */
type OutputEntry =
  | { kind: 'line'; offset: number; text: string }
  | { kind: 'gap'; id: number };

// Liveness vs outcome, matching the Work scope panel: a running handle is a
// green live dot (alive, not a success check); a terminal handle shows its
// exit outcome — ✓ on a clean exit, ✗ on non-zero/kill.
function bashGlyph(insp: BashHandleInspection): { glyph: string; cls: string; title: string } {
  switch (insp.state) {
    case 'running':
      return { glyph: '●', cls: 'pinsp-glyph--live', title: 'running' };
    case 'kill_pending_kernel':
      return { glyph: '⏱', cls: 'pinsp-glyph--warn', title: 'kill pending (kernel)' };
    case 'tombstoned': {
      const success = insp.exit_code === 0 && insp.signal_number == null;
      if (success) return { glyph: '✓', cls: 'pinsp-glyph--ok', title: 'exited 0' };
      const title =
        insp.signal_number != null
          ? `killed (signal ${insp.signal_number})`
          : insp.exit_code != null
            ? `exited ${insp.exit_code}`
            : 'exited (unknown status)';
      return { glyph: '✗', cls: 'pinsp-glyph--err', title };
    }
  }
}

function isTerminal(state: BashHandleState): boolean {
  return state === 'tombstoned';
}

function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

/** Mirror of WorkScopePanel's helper — proportional memory in human units. */
function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** A live resource metric rendered as unavailable when null/undefined — a real
 *  capability gap, not a 0 sample (REQ-PINSP-004 / REQ-PINSP-008). */
function metric(value: number | null | undefined, render: (v: number) => string): string {
  return value == null ? '—' : render(value);
}

/**
 * Polls one handle's inspection snapshot while open and not terminal.
 *
 * Returns the latest full snapshot (identity/state/resources), the accumulated
 * output entries, and a `status` describing the health of the poll. Output is
 * append-only across polls: the seed fetch (no `since`) establishes the tail,
 * then each poll passes `since = end_offset` and appends only the new lines,
 * inserting a gap marker when a poll reports `truncated_before`.
 *
 * The inspect endpoint samples resources (a CPU sleep + /proc reads), so a
 * request can outlast the poll interval. An in-flight gate keeps at most one
 * poll outstanding: the interval skips issuing a new fetch while one is
 * pending. With no overlap the append + `sinceRef` advance are strictly
 * ordered — an out-of-order resolution can never duplicate lines or move the
 * cursor backwards.
 *
 * Poll health is surfaced even once a snapshot exists (REQ-PINSP-006): a
 * failure after the seed leaves the last-known snapshot in place but marks the
 * status `stale` so the operator knows the data may be out of date. A definitive
 * 404 (the handle is gone) is terminal: polling stops and the status becomes
 * `gone`.
 */
type InspectStatus = 'ok' | 'loading-failed' | 'stale' | 'gone';

function useHandleInspection(scopeKey: string, handleId: string) {
  const [snapshot, setSnapshot] = useState<BashHandleInspection | null>(null);
  const [entries, setEntries] = useState<OutputEntry[]>([]);
  const [status, setStatus] = useState<InspectStatus>('ok');

  // Next `since` to request. `undefined` until the seed fetch lands, after
  // which it tracks the last `end_offset`. Held in a ref so the poll callback
  // reads the freshest cursor without being re-created each render.
  const sinceRef = useRef<number | undefined>(undefined);
  const gapIdRef = useRef(0);
  // Once the snapshot reports terminal — or the handle 404s — we never poll
  // again (REQ-PINSP-006).
  const terminalRef = useRef(false);
  // In-flight gate: true while an inspect fetch is outstanding. The recurring
  // poll skips issuing a new fetch while this is set, so at most one poll is
  // ever in flight. The inspect endpoint can be slower than the poll interval
  // (it sleeps to sample CPU); without the gate, interval N+1 could start with
  // the same `sinceRef` as N still in flight, and an out-of-order resolution
  // would append duplicate lines and move `sinceRef` backwards. Cleared in a
  // `.finally`, and reset on unmount / handle change by the seed effect.
  const inFlightRef = useRef(false);
  // Gate ownership token. The gate is shared, but a poll's `.finally` must only
  // clear it if the poll still owns the current generation. When the target
  // switches, the seed effect bumps `generationRef`; a still-pending poll for
  // the OLD target then resolves with a stale token and must NOT clear the gate
  // — otherwise it could unlock the NEW target's in-flight poll and let the next
  // interval issue a second overlapping fetch (reintroducing the duplicate /
  // cursor-regression race the gate prevents).
  const generationRef = useRef(0);

  const applyResponse = useCallback((insp: BashHandleInspection) => {
    setStatus('ok');
    setSnapshot(insp);
    if (isTerminal(insp.state)) terminalRef.current = true;

    const window = insp.output;
    const newEntries: OutputEntry[] = [];
    // A gap means earlier output is unavailable; surface it once, before the
    // lines from this poll. `truncated_before` is authoritative on the seed too:
    // a fresh handle with no eviction reports `false`, while a handle that has
    // already evicted older output (or a tombstone retaining only a final tail)
    // reports `true` — a real signal that earlier output is gone.
    if (window.truncated_before) {
      newEntries.push({ kind: 'gap', id: gapIdRef.current++ });
    }
    for (const line of window.lines) {
      newEntries.push({ kind: 'line', offset: line.offset, text: line.bytes });
    }
    sinceRef.current = window.end_offset;
    if (newEntries.length > 0) {
      setEntries((prev) => (prev.length === 0 ? newEntries : [...prev, ...newEntries]));
    }
  }, []);

  // Translate a rejected fetch into a status. A 404 is definitive: the handle
  // is gone, so stop polling. Any other failure is treated as transient — the
  // last-known snapshot (if any) is retained and marked stale; with no snapshot
  // yet the seed itself failed.
  const applyError = useCallback((err: unknown, hadSnapshot: boolean) => {
    if (err instanceof NotFoundError) {
      terminalRef.current = true;
      setStatus('gone');
    } else {
      setStatus(hadSnapshot ? 'stale' : 'loading-failed');
    }
  }, []);

  // Seed + reset when the target handle changes.
  useEffect(() => {
    let cancelled = false;
    sinceRef.current = undefined;
    terminalRef.current = false;
    gapIdRef.current = 0;
    inFlightRef.current = false;
    // New target → new generation. Any poll still pending for the prior target
    // now owns a stale token and will skip clearing the gate when it resolves.
    generationRef.current += 1;
    setSnapshot(null);
    setEntries([]);
    setStatus('ok');

    api
      .getBashHandleInspection(scopeKey, handleId)
      .then((insp) => {
        if (!cancelled) applyResponse(insp);
      })
      .catch((err: unknown) => {
        if (!cancelled) applyError(err, false);
      });

    return () => {
      cancelled = true;
    };
  }, [scopeKey, handleId, applyResponse, applyError]);

  // Poll loop: ~1s while open and not terminal. Self-limiting — the interval is
  // cleared on unmount, on handle change, and once the handle goes terminal.
  // Gating on `snapshot` (re)starts it after the seed lands and lets the
  // terminal check below short-circuit it.
  const polling = snapshot != null && !isTerminal(snapshot.state);
  useEffect(() => {
    if (!polling) return;
    let cancelled = false;
    const id = window.setInterval(() => {
      if (terminalRef.current || inFlightRef.current) return;
      inFlightRef.current = true;
      // Capture the generation this poll owns. Only clear the gate in `.finally`
      // if it still matches — a target switch bumps `generationRef`, so a poll
      // for a now-replaced target cannot unlock the new target's gate.
      const generation = generationRef.current;
      api
        .getBashHandleInspection(scopeKey, handleId, sinceRef.current)
        .then((insp) => {
          if (!cancelled) applyResponse(insp);
        })
        .catch((err: unknown) => {
          if (!cancelled) applyError(err, true);
        })
        .finally(() => {
          if (generation === generationRef.current) inFlightRef.current = false;
        });
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [polling, scopeKey, handleId, applyResponse, applyError]);

  return { snapshot, entries, status };
}

interface ProcessInspectorPanelProps {
  scopeKey: string;
  handleId: string;
  /** Close handler for the header button (REQ-PINSP-007). */
  onClose?: () => void;
  /** When true, render with the inline split-pane chrome (no overlay). */
  inline?: boolean;
}

export function ProcessInspectorPanel({
  scopeKey,
  handleId,
  onClose,
  inline,
}: ProcessInspectorPanelProps) {
  const { snapshot, entries, status } = useHandleInspection(scopeKey, handleId);

  // Tick once a second so a live handle's elapsed time advances between polls.
  // A `gone` handle no longer has a live process, so freeze the elapsed clock.
  const [now, setNow] = useState(() => Date.now());
  const live = snapshot != null && !isTerminal(snapshot.state) && status !== 'gone';
  useEffect(() => {
    if (!live) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [live]);

  // Autoscroll-with-pause (REQ-PINSP-008): follow the tail by default; pause
  // when the user scrolls up; resume when they return to within a small slack
  // of the bottom.
  const outputRef = useRef<HTMLDivElement | null>(null);
  const [following, setFollowing] = useState(true);
  const followingRef = useRef(true);
  followingRef.current = following;

  const onScroll = useCallback(() => {
    const el = outputRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= 24;
    if (atBottom !== followingRef.current) setFollowing(atBottom);
  }, []);

  useLayoutEffect(() => {
    if (!following) return;
    const el = outputRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries, following]);

  const glyph = snapshot ? bashGlyph(snapshot) : null;
  const label = snapshot?.label ?? null;
  const cmd = snapshot?.cmd ?? '';
  const terminal = snapshot != null && isTerminal(snapshot.state);

  const elapsed = snapshot
    ? snapshot.duration_ms != null
      ? formatDuration(snapshot.duration_ms)
      : formatDuration(Math.max(0, now - Date.parse(snapshot.started_at)))
    : null;

  const exitCause = (() => {
    if (!snapshot || !terminal) return null;
    if (snapshot.signal_number != null) return `signal ${snapshot.signal_number}`;
    if (snapshot.exit_code != null) return `exit ${snapshot.exit_code}`;
    return 'ended';
  })();

  const resources = snapshot?.resources;

  return (
    <div
      className={`pinsp-panel${inline ? ' pinsp-panel--inline' : ''}`}
      data-testid="process-inspector-panel"
    >
      <div className="pinsp-header">
        {glyph && (
          <span className={`pinsp-glyph ${glyph.cls}`} title={glyph.title} aria-label={glyph.title}>
            {glyph.glyph}
          </span>
        )}
        <span className="pinsp-title" title={cmd}>
          {label ?? cmd ?? handleId}
        </span>
        {onClose && (
          <button
            type="button"
            className="pinsp-close"
            onClick={onClose}
            aria-label="Close process inspector"
          >
            ×
          </button>
        )}
      </div>

      {status === 'loading-failed' && !snapshot && (
        <div className="pinsp-empty pinsp-empty--error">inspection failed to load</div>
      )}
      {/* The first inspect request 404'd (e.g. a persisted inspector URL opened
          after a Phoenix restart dropped the handle table). There is no
          last-known output to show, but the user must still be told the handle
          is gone and that polling has stopped. */}
      {status === 'gone' && !snapshot && (
        <div className="pinsp-empty pinsp-empty--error" role="status">
          handle no longer exists
        </div>
      )}
      {status === 'ok' && !snapshot && <div className="pinsp-empty">loading&hellip;</div>}

      {snapshot && (status === 'stale' || status === 'gone') && (
        <div
          className={`pinsp-stale${status === 'gone' ? ' pinsp-stale--gone' : ''}`}
          role="status"
        >
          <span className="pinsp-stale-glyph" aria-hidden="true">
            {status === 'gone' ? '✗' : '⚠'}
          </span>
          <span className="pinsp-stale-text">
            {status === 'gone'
              ? 'handle no longer exists — updates stopped, last-known output below'
              : 'updates stalled — data below may be stale'}
          </span>
        </div>
      )}

      {snapshot && (
        <>
          <div className="pinsp-identity">
            <div className="pinsp-id-line">
              <span className="pinsp-id-key">cmd</span>
              <span className="pinsp-id-val pinsp-id-val--cmd" title={cmd}>{cmd}</span>
            </div>
            {label && (
              <div className="pinsp-id-line">
                <span className="pinsp-id-key">label</span>
                <span className="pinsp-id-val">{label}</span>
              </div>
            )}
            <div className="pinsp-id-line">
              <span className="pinsp-id-key">id</span>
              <span className="pinsp-id-val">{snapshot.handle_id}</span>
            </div>
            {snapshot.pid != null && (
              <div className="pinsp-id-line">
                <span className="pinsp-id-key">pid</span>
                <span className="pinsp-id-val">
                  {snapshot.pid}
                  {snapshot.pgid != null ? ` (pgid ${snapshot.pgid})` : ''}
                </span>
              </div>
            )}
            {terminal && exitCause && (
              <div className="pinsp-id-line">
                <span className="pinsp-id-key">exit</span>
                <span className="pinsp-id-val">{exitCause}</span>
              </div>
            )}
            <div className="pinsp-id-line">
              <span className="pinsp-id-key">{terminal ? 'ran for' : 'elapsed'}</span>
              <span className="pinsp-id-val">{elapsed}</span>
            </div>
          </div>

          <div className="pinsp-resources" aria-label="Resource sample">
            <span className="pinsp-res-item" title="Summed CPU over the process group">
              <span className="pinsp-res-key">cpu</span>
              <span className="pinsp-res-val">{metric(resources?.cpu_pct, (v) => `${v.toFixed(1)}%`)}</span>
            </span>
            <span className="pinsp-res-item" title="Proportional, shared-aware memory (PSS / phys_footprint)">
              <span className="pinsp-res-key">mem</span>
              <span className="pinsp-res-val">{metric(resources?.memory_bytes, formatBytes)}</span>
            </span>
            <span className="pinsp-res-item" title="Live processes in the group">
              <span className="pinsp-res-key">procs</span>
              <span className="pinsp-res-val">{metric(resources?.process_count, (v) => String(v))}</span>
            </span>
            {terminal && <span className="pinsp-res-item pinsp-res-item--muted">no process group</span>}
          </div>

          <div className="pinsp-output" ref={outputRef} onScroll={onScroll}>
            {entries.length === 0 && !snapshot?.output.partial ? (
              <div className="pinsp-output-empty">no output</div>
            ) : (
              <>
                {entries.map((e) =>
                  e.kind === 'gap' ? (
                    <div key={`gap-${e.id}`} className="pinsp-output-gap">
                      … output truncated …
                    </div>
                  ) : (
                    <div key={`line-${e.offset}`} className="pinsp-output-line">
                      {e.text}
                    </div>
                  ),
                )}
                {/* The live trailing partial — bytes written since the last
                    newline. Transient (replaced each poll, never appended to
                    the offset-keyed entries); rendered as an in-progress line
                    so un-newlined output is visible without waiting for a
                    flush (REQ-PINSP-003). */}
                {snapshot?.output.partial && (
                  <div
                    className="pinsp-output-line pinsp-output-line--partial"
                    title="in-progress line (no newline yet)"
                  >
                    {snapshot.output.partial}
                  </div>
                )}
              </>
            )}
          </div>
          {!following && (
            <button
              type="button"
              className="pinsp-follow-resume"
              onClick={() => {
                const el = outputRef.current;
                if (el) el.scrollTop = el.scrollHeight;
                setFollowing(true);
              }}
            >
              ↓ Resume autoscroll
            </button>
          )}
        </>
      )}
    </div>
  );
}
