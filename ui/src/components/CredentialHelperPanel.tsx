import { useEffect, useRef, useState } from 'react';

interface CredentialHelperPanelProps {
  onClose: () => void;
}

type PanelState = 'connecting' | 'running' | 'complete' | 'error';

interface ErrorInfo {
  exit_code: number | null;
  stderr: string;
}

export function CredentialHelperPanel({ onClose }: CredentialHelperPanelProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [panelState, setPanelState] = useState<PanelState>('connecting');
  const [errorInfo, setErrorInfo] = useState<ErrorInfo | null>(null);
  const outputRef = useRef<HTMLDivElement>(null);
  const esRef = useRef<EventSource | null>(null);
  const doneRef = useRef(false);

  useEffect(() => {
    const es = new EventSource('/api/credential-helper/run');
    esRef.current = es;

    es.addEventListener('message', (event) => {
      try {
        const data = JSON.parse(event.data);
        if (data.type === 'line') {
          setPanelState('running');
          setLines(prev => [...prev, data.text]);
        } else if (data.type === 'complete') {
          doneRef.current = true;
          setPanelState('complete');
          es.close();
        } else if (data.type === 'error') {
          doneRef.current = true;
          setPanelState('error');
          setErrorInfo({ exit_code: data.exit_code ?? null, stderr: data.stderr ?? '' });
          es.close();
        }
      } catch {
        // ignore parse errors
      }
    });

    es.onerror = () => {
      if (!doneRef.current) {
        setPanelState('error');
        setErrorInfo({ exit_code: null, stderr: 'Connection to server lost.' });
      }
      es.close();
    };

    return () => {
      es.close();
    };
  }, []);

  // Auto-scroll output to bottom as lines arrive
  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [lines]);

  const statusLabel = {
    connecting: 'Connecting...',
    running: 'Authenticating...',
    complete: 'Authentication complete',
    error: 'Authentication failed',
  }[panelState];

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal credential-helper-panel" onClick={e => e.stopPropagation()}>
        <h3>Authenticate</h3>
        <p className="credential-helper-status">{statusLabel}</p>
        {lines.length > 0 && (
          <div className="credential-helper-output" ref={outputRef}>
            {lines.map((line, i) => (
              <div key={i} className="credential-helper-line">
                {line || '\u00a0'}
              </div>
            ))}
          </div>
        )}
        {panelState === 'error' && errorInfo && (
          <div className="credential-helper-error">
            {errorInfo.exit_code !== null && (
              <div>Exit code: {errorInfo.exit_code}</div>
            )}
            {errorInfo.stderr && (
              <pre className="credential-helper-stderr">{errorInfo.stderr}</pre>
            )}
          </div>
        )}
        <div className="modal-actions">
          <button
            className={panelState === 'complete' ? 'btn-primary' : 'btn-secondary'}
            onClick={onClose}
          >
            {panelState === 'complete' ? 'Done' : 'Cancel'}
          </button>
        </div>
      </div>
    </div>
  );
}
