import { useState, useCallback, useLayoutEffect, useRef } from 'react';
import { copyToClipboard } from '../utils/clipboard';

interface CopyButtonProps {
  text: string;
  className?: string;
  title?: string;
  disabled?: boolean;
}

function CopyIcon() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <rect x="6" y="3" width="7" height="9" rx="1.3" />
      <rect x="3" y="6" width="7" height="7" rx="1.3" />
    </svg>
  );
}

function CopiedIcon() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
    </svg>
  );
}

export function CopyButton({ text, className = '', title = 'Copy to clipboard', disabled = false }: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const resetTimeout = useRef<number | null>(null);
  const activeCopy = useRef<AbortController | null>(null);

  useLayoutEffect(() => {
    activeCopy.current?.abort();
    activeCopy.current = null;
    setCopied(false);
    if (resetTimeout.current !== null) {
      window.clearTimeout(resetTimeout.current);
      resetTimeout.current = null;
    }

    return () => {
      activeCopy.current?.abort();
      activeCopy.current = null;
      if (resetTimeout.current !== null) {
        window.clearTimeout(resetTimeout.current);
        resetTimeout.current = null;
      }
    };
  }, [text, disabled]);

  const handleCopy = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (disabled) return;

    activeCopy.current?.abort();
    if (resetTimeout.current !== null) {
      window.clearTimeout(resetTimeout.current);
      resetTimeout.current = null;
    }
    const controller = new AbortController();
    activeCopy.current = controller;
    setCopied(false);

    if (await copyToClipboard(text, controller.signal) && !controller.signal.aborted) {
      setCopied(true);
      resetTimeout.current = window.setTimeout(() => {
        if (!controller.signal.aborted) setCopied(false);
        resetTimeout.current = null;
      }, 2000);
    }
  }, [disabled, text]);

  return (
    <button
      type="button"
      className={`copy-btn ${copied ? 'copied' : ''} ${className}`}
      onClick={handleCopy}
      title={copied ? 'Copied!' : title}
      aria-label={copied ? 'Copied!' : title}
      disabled={disabled}
    >
      {copied ? <CopiedIcon /> : <CopyIcon />}
    </button>
  );
}
