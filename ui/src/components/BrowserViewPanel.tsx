/**
 * BrowserViewPanel — live mirror of the conversation's headless Chromium
 * (REQ-BT-018).
 *
 * Connects to `GET /api/conversations/:id/browser-view` over a binary
 * WebSocket. The wire protocol mirrors `src/api/browser_view.rs`:
 *
 *   byte 0 = 0x00 → JPEG frame: [0x00][u32be jpeg_length][jpeg bytes...]
 *   byte 0 = 0x01 → URL change: [0x01][utf-8 url string]
 *   byte 0 = 0x02 → status:     [0x02][utf-8 status string]
 *                                  "no-session" | "started" | "ended" | "error: ..."
 *
 * The panel is **view-only by design**. Pointer-events are disabled on the
 * canvas so a user clicking on the surface never confuses themselves into
 * thinking the click registered against the agent's page. See the locked-in
 * non-goals in `tasks/05001-*.md` and `specs/browser-tool/requirements.md`
 * (REQ-BT-018).
 *
 * Reconnect strategy mirrors TerminalPanel: on transient drops, retry with
 * a small backoff. The "no-session" status is treated as "agent hasn't
 * touched the browser yet" — we keep the connection open and let the user
 * see the placeholder until the next attempt finds a session.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

const TAG_FRAME = 0x00;
const TAG_URL = 0x01;
const TAG_STATUS = 0x02;

/** Build the WebSocket URL for a conversation's browser-view endpoint. */
function browserViewWsUrl(conversationId: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}/api/conversations/${conversationId}/browser-view`;
}

type Status =
  | { kind: 'connecting' }
  | { kind: 'no-session' }
  | { kind: 'live' }
  | { kind: 'ended' }
  | { kind: 'error'; message: string };

interface BrowserViewPanelProps {
  conversationId: string;
  /** Click handler for the close button in the header. */
  onClose?: () => void;
  /** When true, render with the inline split-pane chrome (no overlay). */
  inline?: boolean;
}

export function BrowserViewPanel({
  conversationId,
  onClose,
  inline,
}: BrowserViewPanelProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimerRef = useRef<number | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>({ kind: 'connecting' });
  /** Bumping this triggers the connect effect to retear and reconnect. */
  const [reconnectNonce, setReconnectNonce] = useState(0);

  const drawJpeg = useCallback((bytes: Uint8Array) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    // The Blob/createImageBitmap path is the cheap path for JPEGs in the
    // browser — avoids round-tripping through a base64 data URL or `<img>`.
    // We hand off to a microtask via the returned promise; if the canvas
    // unmounts mid-decode we just drop the result.
    const blob = new Blob([bytes as BlobPart], { type: 'image/jpeg' });
    createImageBitmap(blob)
      .then((bitmap) => {
        const c = canvasRef.current;
        if (!c) {
          bitmap.close();
          return;
        }
        // Match canvas backing store to the bitmap so we don't re-scale on
        // every frame; CSS will fit it into the panel via object-fit.
        if (c.width !== bitmap.width || c.height !== bitmap.height) {
          c.width = bitmap.width;
          c.height = bitmap.height;
        }
        const ctx = c.getContext('2d');
        if (!ctx) {
          bitmap.close();
          return;
        }
        ctx.drawImage(bitmap, 0, 0);
        bitmap.close();
      })
      .catch(() => {
        // Decode failure on a single frame — skip it, the next one will be
        // along shortly. Logging at info would spam.
      });
  }, []);

  // Connect on mount, on conversation change, on reconnect nonce change.
  useEffect(() => {
    let cancelled = false;

    const ws = new WebSocket(browserViewWsUrl(conversationId));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    setStatus({ kind: 'connecting' });

    ws.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      const data = new Uint8Array(event.data);
      if (data.length === 0) return;
      const tag = data[0];
      if (tag === TAG_FRAME) {
        // [tag][u32be len][jpeg bytes]
        if (data.length < 5) return;
        const b1 = data[1] ?? 0;
        const b2 = data[2] ?? 0;
        const b3 = data[3] ?? 0;
        const b4 = data[4] ?? 0;
        const len = (b1 << 24) | (b2 << 16) | (b3 << 8) | b4;
        const jpeg = data.subarray(5, 5 + len);
        drawJpeg(jpeg);
        // Receiving a frame implies the screencast is live, regardless of
        // what status arrived earlier (or didn't).
        setStatus((prev) => (prev.kind === 'live' ? prev : { kind: 'live' }));
      } else if (tag === TAG_URL) {
        const text = new TextDecoder('utf-8').decode(data.subarray(1));
        setUrl(text);
      } else if (tag === TAG_STATUS) {
        const text = new TextDecoder('utf-8').decode(data.subarray(1));
        if (text === 'no-session') {
          setStatus({ kind: 'no-session' });
        } else if (text === 'started') {
          setStatus({ kind: 'live' });
        } else if (text === 'ended') {
          setStatus({ kind: 'ended' });
        } else if (text.startsWith('error:')) {
          setStatus({ kind: 'error', message: text.slice(6).trim() });
        }
      }
    };

    ws.onerror = () => {
      if (cancelled) return;
      setStatus({ kind: 'error', message: 'connection error' });
    };

    ws.onclose = () => {
      if (cancelled) return;
      // If we never flipped to 'live' or 'ended', this is a transient drop;
      // schedule a quiet retry so the panel reconnects when the agent's
      // session comes online (e.g. after the first browser_* tool fires).
      setStatus((prev) => {
        if (prev.kind === 'live') return { kind: 'ended' };
        return prev;
      });
      reconnectTimerRef.current = window.setTimeout(() => {
        reconnectTimerRef.current = null;
        setReconnectNonce((n) => n + 1);
      }, 1500);
    };

    return () => {
      cancelled = true;
      if (reconnectTimerRef.current !== null) {
        window.clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
      // Unbind handlers BEFORE close() so the onclose for a WS that's still
      // mid-handshake can't race with a fresh effect run and clobber the
      // freshly-set state. Same dance TerminalPanel does for the same reason.
      ws.onopen = null;
      ws.onclose = null;
      ws.onerror = null;
      ws.onmessage = null;
      try {
        ws.close();
      } catch {
        // already closed — no-op
      }
      wsRef.current = null;
    };
  }, [conversationId, reconnectNonce, drawJpeg]);

  return (
    <div
      className={`browser-view-panel${inline ? ' browser-view-panel--inline' : ''}`}
      data-testid="browser-view-panel"
    >
      <div className="browser-view-panel__header">
        <span
          className="browser-view-panel__status"
          data-status={status.kind}
          aria-label={`Browser view status: ${status.kind}`}
          title={
            status.kind === 'error'
              ? `Error: ${status.message}`
              : status.kind === 'no-session'
                ? 'Agent has not opened the browser yet'
                : status.kind === 'ended'
                  ? 'Browser session ended'
                  : status.kind === 'connecting'
                    ? 'Connecting…'
                    : 'Live'
          }
        />
        <span className="browser-view-panel__url" title={url ?? ''}>
          {url ?? '—'}
        </span>
        <span
          className="browser-view-panel__readonly"
          title="View-only mirror. The agent drives; clicks here have no effect."
        >
          view-only
        </span>
        {onClose && (
          <button
            type="button"
            className="browser-view-panel__close"
            onClick={onClose}
            aria-label="Close browser view"
          >
            ×
          </button>
        )}
      </div>
      <div className="browser-view-panel__stage">
        <canvas
          ref={canvasRef}
          className="browser-view-panel__canvas"
          // Disable pointer events so clicks on the canvas can't be confused
          // for actual page input (REQ-BT-018-NG-INPUT). The header's title
          // tooltip explains why.
          style={{ pointerEvents: 'none' }}
        />
        {status.kind !== 'live' && (
          <div className="browser-view-panel__overlay" role="status">
            {status.kind === 'connecting' && 'Connecting…'}
            {status.kind === 'no-session' && (
              <>
                <div>No browser yet.</div>
                <div className="browser-view-panel__overlay-sub">
                  Will appear when the agent uses a browser tool.
                </div>
              </>
            )}
            {status.kind === 'ended' && 'Browser session ended.'}
            {status.kind === 'error' && `Error: ${status.message}`}
          </div>
        )}
      </div>
    </div>
  );
}
