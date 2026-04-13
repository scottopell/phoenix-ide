import { useEffect, useRef, useState, useCallback } from 'react';

interface CredentialHelperPanelProps {
  /** When true, connects to the SSE endpoint and runs the helper */
  active: boolean;
  /** Called when the user explicitly dismisses after completion/error */
  onDismiss: () => void;
}

type StripState = 'connecting' | 'running' | 'complete' | 'error';

interface ErrorInfo {
  exit_code: number | null;
  stderr: string;
}

export function CredentialHelperPanel({ active, onDismiss }: CredentialHelperPanelProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [stripState, setStripState] = useState<StripState>('connecting');
  const [errorInfo, setErrorInfo] = useState<ErrorInfo | null>(null);
  const [expanded, setExpanded] = useState(true);
  const outputRef = useRef<HTMLDivElement>(null);
  const doneRef = useRef(false);

  useEffect(() => {
    if (!active) {
      // Reset when deactivated
      setLines([]);
      setStripState('connecting');
      setErrorInfo(null);
      setExpanded(true);
      doneRef.current = false;
      return;
    }

    const es = new EventSource('/api/credential-helper/run');
    doneRef.current = false;

    es.addEventListener('message', (event) => {
      try {
        const data = JSON.parse(event.data);
        if (data.type === 'line') {
          setStripState('running');
          setLines(prev => [...prev, data.text]);
        } else if (data.type === 'complete') {
          doneRef.current = true;
          setStripState('complete');
          setExpanded(false);
          es.close();
        } else if (data.type === 'error') {
          doneRef.current = true;
          setStripState('error');
          setErrorInfo({ exit_code: data.exit_code ?? null, stderr: data.stderr ?? '' });
          es.close();
        }
      } catch {
        // ignore parse errors
      }
    });

    es.onerror = () => {
      if (!doneRef.current) {
        setStripState('error');
        setErrorInfo({ exit_code: null, stderr: 'Connection to server lost.' });
      }
      es.close();
    };

    return () => {
      es.close();
    };
  }, [active]);

  // Auto-scroll output
  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [lines]);

  const handleDismiss = useCallback(() => {
    onDismiss();
  }, [onDismiss]);

  if (!active && stripState === 'connecting') {
    return null;
  }

  const stateIndicator = {
    connecting: { symbol: '...', label: 'Connecting', className: 'running' },
    running:    { symbol: '...', label: 'Authenticating', className: 'running' },
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
            onClick={(e) => { e.stopPropagation(); handleDismiss(); }}
            onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); handleDismiss(); } }}
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
            </div>
          )}
        </div>
      )}
    </div>
  );
}
