import { useState, useCallback, useEffect, useRef } from 'react';
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
      <rect x="5" y="3" width="8" height="10" rx="1.5" />
      <path d="M3 6.5V4.5A1.5 1.5 0 0 1 4.5 3H7" />
      <path d="M3 6.5v5A1.5 1.5 0 0 0 4.5 13H7" />
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

  useEffect(() => {
    setCopied(false);
    if (resetTimeout.current !== null) {
      window.clearTimeout(resetTimeout.current);
      resetTimeout.current = null;
    }
  }, [text]);

  useEffect(() => {
    return () => {
      if (resetTimeout.current !== null) {
        window.clearTimeout(resetTimeout.current);
      }
    };
  }, []);

  const handleCopy = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (disabled) return;
    if (await copyToClipboard(text)) {
      setCopied(true);
      if (resetTimeout.current !== null) {
        window.clearTimeout(resetTimeout.current);
      }
      resetTimeout.current = window.setTimeout(() => {
        setCopied(false);
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
