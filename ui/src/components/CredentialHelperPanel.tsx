import { useEffect, useRef, useState } from 'react';

interface CredentialHelperPanelProps {
  /** When true, connects to the SSE endpoint and runs the helper */
  active: boolean;
  /** Called when the user explicitly dismisses after completion/error */
  onDismiss: () => void;
}

type StripState = 'connecting' | 'running' | 'complete' | 'error' | 'retrying';

interface ErrorInfo {
  exit_code: number | null;
  stderr: string;
}

const MAX_RETRIES = 3;

export function CredentialHelperPanel({ active, onDismiss }: CredentialHelperPanelProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [stripState, setStripState] = useState<StripState>('connecting');
  const [errorInfo, setErrorInfo] = useState<ErrorInfo | null>(null);
  const [expanded, setExpanded] = useState(true);
  const [retryDisplay, setRetryDisplay] = useState(0);
  const outputRef = useRef<HTMLDivElement>(null);
  const doneRef = useRef(false);
  const retryCountRef = useRef(0);
  const esRef = useRef<EventSource | null>(null);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const connect = () => {
    if (esRef.current) {
      esRef.current.close();
    }

    const es = new EventSource('/api/credential-helper/run');
    esRef.current = es;
    doneRef.current = false;

    es.addEventListener('message', (event) => {
      try {
        const data = JSON.parse(event.data);
        if (data.type === 'line') {
          setStripState('running');
          setLines(prev => [...prev, data.text]);
        } else if (data.type === 'complete') {
          doneRef.current = true;
          retryCountRef.current = 0;
          setRetryDisplay(0);
          setStripState('complete');
          setExpanded(false);
          es.close();
        } else if (data.type === 'error') {
          doneRef.current = true;
          es.close();
          if (retryCountRef.current < MAX_RETRIES) {
            retryCountRef.current += 1;
            setRetryDisplay(retryCountRef.current);
            setLines([]);
            setStripState('retrying');
            retryTimerRef.current = setTimeout(() => {
              connect();
            }, 2000);
          } else {
            setStripState('error');
            setErrorInfo({ exit_code: data.exit_code ?? null, stderr: data.stderr ?? '' });
          }
        }
      } catch {
        // ignore parse errors
      }
    });

    es.onerror = () => {
      if (!doneRef.current) {
        es.close();
        if (retryCountRef.current < MAX_RETRIES) {
          retryCountRef.current += 1;
          setRetryDisplay(retryCountRef.current);
          setLines([]);
          setStripState('retrying');
          retryTimerRef.current = setTimeout(() => {
            connect();
          }, 2000);
        } else {
          setStripState('error');
          setErrorInfo({ exit_code: null, stderr: 'Connection to server lost.' });
        }
      } else {
        es.close();
      }
    };
  };

  useEffect(() => {
    if (!active) {
      // Reset when deactivated
      if (esRef.current) {
        esRef.current.close();
        esRef.current = null;
      }
      if (retryTimerRef.current) {
        clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
      setLines([]);
      setStripState('connecting');
      setErrorInfo(null);
      setExpanded(true);
      doneRef.current = false;
      retryCountRef.current = 0;
      setRetryDisplay(0);
      return;
    }

    connect();

    return () => {
      if (esRef.current) {
        esRef.current.close();
        esRef.current = null;
      }
      if (retryTimerRef.current) {
        clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
    };
  }, [active]);

  // Auto-scroll output
  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [lines]);

  if (!active && stripState === 'connecting') {
    return null;
  }

  const stateIndicator = {
    connecting: { symbol: '...', label: 'Connecting', className: 'running' },
    running:    { symbol: '...', label: 'Authenticating', className: 'running' },
    retrying:   { symbol: '...', label: `Retrying (${retryDisplay}/${MAX_RETRIES})...`, className: 'running' },
    complete:   { symbol: '\u2713', label: 'Authenticated', className: 'valid' },
    error:      { symbol: '\u2717', label: 'Auth failed', className: 'error' },
  }[stripState];

  return (
    <div className={`auth-strip ${stateIndicator.className}${expanded ? ' expanded' : ''}`}>
      <button
        className="auth-strip-header"
        onClick={() => setExpanded(!expanded)}
        type="button"
      >
        <span className="auth-strip-indicator">{stateIndicator.symbol}</span>
        <span className="auth-strip-label">{stateIndicator.label}</span>
        <span className="auth-strip-chevron">{expanded ? '\u25B4' : '\u25BE'}</span>
        {(stripState === 'complete' || stripState === 'error') && (
          <span
            className="auth-strip-dismiss"
            role="button"
            tabIndex={0}
            onClick={(e) => { e.stopPropagation(); onDismiss(); }}
            onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); onDismiss(); } }}
          >
            dismiss
          </span>
        )}
      </button>
      {expanded && (
        <div className="auth-strip-body">
          {lines.length > 0 && (
            <div className="auth-strip-output" ref={outputRef}>
              {lines.map((line, i) => (
                <div key={i} className="auth-strip-line">
                  {line || '\u00a0'}
                </div>
              ))}
            </div>
          )}
          {stripState === 'error' && errorInfo && (
            <div className="auth-strip-error">
              {errorInfo.exit_code !== null && (
                <div>Exit code: {errorInfo.exit_code}</div>
              )}
              {errorInfo.stderr && (
                <pre className="auth-strip-stderr">{errorInfo.stderr}</pre>
              )}
              <button
                className="auth-strip-retry"
                type="button"
                onClick={() => {
                  retryCountRef.current = 0;
                  setRetryDisplay(0);
                  setLines([]);
                  setErrorInfo(null);
                  setStripState('connecting');
                  connect();
                }}
              >
                Retry
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
