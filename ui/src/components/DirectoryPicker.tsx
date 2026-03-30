import { useState, useEffect, useRef, useCallback } from 'react';
import { Folder, ChevronRight } from 'lucide-react';
import { api } from '../api';
import { Skeleton } from './Skeleton';
import type { DirStatus } from './SettingsFields';

interface DirectoryEntry {
  name: string;
  is_dir: boolean;
}

interface DirectoryPickerProps {
  value: string;
  onChange: (path: string) => void;
  onStatusChange?: (status: DirStatus) => void;
  placeholder?: string;
  className?: string;
}

const STATUS_CLASS_MAP: Record<DirStatus, string> = {
  checking: 'status-checking',
  exists: 'status-ok',
  'will-create': 'status-create',
  invalid: 'status-error',
};

const STATUS_ICON_MAP: Record<DirStatus, { icon: string; title: string }> = {
  checking: { icon: '...', title: 'Checking path...' },
  exists: { icon: '\u2713', title: 'Directory exists' },
  'will-create': { icon: '+', title: 'Directory will be created' },
  invalid: { icon: '\u2717', title: 'Invalid path' },
};

function parsePath(value: string): { parentPath: string; partial: string } {
  if (!value || !value.startsWith('/')) {
    return { parentPath: '/', partial: '' };
  }
  if (value.endsWith('/')) {
    const parent = value.length > 1 ? value.slice(0, -1) : '/';
    return { parentPath: parent, partial: '' };
  }
  const lastSlash = value.lastIndexOf('/');
  const parent = value.substring(0, lastSlash) || '/';
  const partial = value.substring(lastSlash + 1);
  return { parentPath: parent, partial };
}

export function DirectoryPicker({ value, onChange, onStatusChange, placeholder = '/path/to/project', className = '' }: DirectoryPickerProps) {
  const [suggestions, setSuggestions] = useState<DirectoryEntry[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [showDropdown, setShowDropdown] = useState(false);
  const [pathStatus, setPathStatus] = useState<DirStatus>(() =>
    value.trim().startsWith('/') ? 'exists' : 'checking'
  );
  const isFirstValidation = useRef(true);

  const abortControllerRef = useRef<AbortController | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const blurTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;

  // Cache fetched entries by parent path to avoid redundant API calls
  const cachedParentRef = useRef<string>('');
  const cachedEntriesRef = useRef<DirectoryEntry[]>([]);

  // Fetch suggestions (debounced 150ms)
  useEffect(() => {
    const trimmed = value.trim();
    if (!trimmed || !trimmed.startsWith('/')) {
      setSuggestions([]);
      return;
    }

    const { parentPath, partial } = parsePath(trimmed);

    // If parent is cached, filter client-side immediately
    if (parentPath === cachedParentRef.current) {
      const filtered = partial
        ? cachedEntriesRef.current.filter(e => e.name.toLowerCase().startsWith(partial.toLowerCase()))
        : cachedEntriesRef.current;
      setSuggestions(filtered);
      setSelectedIndex(-1);
      return;
    }

    setIsLoading(true);
    const timeoutId = setTimeout(() => {
      // Cancel previous in-flight request
      if (abortControllerRef.current) {
        abortControllerRef.current.abort();
      }
      const controller = new AbortController();
      abortControllerRef.current = controller;

      api.listDirectory(parentPath, controller.signal)
        .then(resp => {
          if (controller.signal.aborted) return;
          const dirs = resp.entries.filter(e => e.is_dir);
          cachedParentRef.current = parentPath;
          cachedEntriesRef.current = dirs;
          const filtered = partial
            ? dirs.filter(e => e.name.toLowerCase().startsWith(partial.toLowerCase()))
            : dirs;
          setSuggestions(filtered);
          setSelectedIndex(-1);
          setIsLoading(false);
        })
        .catch(err => {
          if (err instanceof DOMException && err.name === 'AbortError') return;
          setSuggestions([]);
          setIsLoading(false);
        });
    }, 150);

    return () => clearTimeout(timeoutId);
  }, [value]);

  // Validate path (debounced 300ms)
  useEffect(() => {
    const trimmed = value.trim();
    if (!trimmed || !trimmed.startsWith('/')) {
      setPathStatus('invalid');
      onStatusChangeRef.current?.('invalid');
      return;
    }

    if (isFirstValidation.current) {
      isFirstValidation.current = false;
    } else {
      setPathStatus('checking');
      onStatusChangeRef.current?.('checking');
    }

    const timeoutId = setTimeout(async () => {
      try {
        const validation = await api.validateCwd(trimmed);
        if (validation.valid) {
          setPathStatus('exists');
          onStatusChangeRef.current?.('exists');
        } else {
          const parentPath = trimmed.substring(0, trimmed.lastIndexOf('/')) || '/';
          const parentValidation = await api.validateCwd(parentPath);
          const status: DirStatus = parentValidation.valid ? 'will-create' : 'invalid';
          setPathStatus(status);
          onStatusChangeRef.current?.(status);
        }
      } catch {
        setPathStatus('invalid');
        onStatusChangeRef.current?.('invalid');
      }
    }, 300);

    return () => clearTimeout(timeoutId);
  }, [value]);

  const handleSelect = useCallback((entry: DirectoryEntry) => {
    const trimmed = value.trim();
    const { parentPath } = parsePath(trimmed);
    const newPath = parentPath === '/' ? `/${entry.name}/` : `${parentPath}/${entry.name}/`;
    onChange(newPath);
    setSelectedIndex(-1);
    // Keep focus for continued drilling
    inputRef.current?.focus();
  }, [value, onChange]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape' && showDropdown) {
      e.preventDefault();
      e.stopPropagation();
      setShowDropdown(false);
      return;
    }

    if (!showDropdown) {
      if (e.key === 'ArrowDown' && suggestions.length > 0) {
        e.preventDefault();
        setShowDropdown(true);
        setSelectedIndex(0);
      }
      return;
    }

    if (suggestions.length === 0) return;

    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setSelectedIndex(prev => (prev + 1) % suggestions.length);
        break;
      case 'ArrowUp':
        e.preventDefault();
        setSelectedIndex(prev => (prev - 1 + suggestions.length) % suggestions.length);
        break;
      case 'Enter': {
        const enterEntry = suggestions[selectedIndex];
        if (selectedIndex >= 0 && enterEntry) {
          e.preventDefault();
          handleSelect(enterEntry);
        }
        break;
      }
      case 'Tab': {
        const tabEntry = suggestions[selectedIndex];
        if (selectedIndex >= 0 && tabEntry) {
          e.preventDefault();
          handleSelect(tabEntry);
        }
        break;
      }
    }
  }, [showDropdown, suggestions, selectedIndex, handleSelect]);

  const handleFocus = useCallback(() => {
    if (blurTimeoutRef.current) {
      clearTimeout(blurTimeoutRef.current);
      blurTimeoutRef.current = null;
    }
    setShowDropdown(true);
  }, []);

  const handleBlur = useCallback(() => {
    blurTimeoutRef.current = setTimeout(() => {
      setShowDropdown(false);
      setSelectedIndex(-1);
    }, 150);
  }, []);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (abortControllerRef.current) abortControllerRef.current.abort();
      if (blurTimeoutRef.current) clearTimeout(blurTimeoutRef.current);
    };
  }, []);

  const statusClass = STATUS_CLASS_MAP[pathStatus];
  const { icon: statusIcon, title: statusTitle } = STATUS_ICON_MAP[pathStatus];
  const dropdownVisible = showDropdown && value.trim().startsWith('/');

  return (
    <div className="directory-picker">
      <div className="path-input-container">
        <input
          ref={inputRef}
          type="text"
          className={`${className} ${statusClass}`}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={handleKeyDown}
          onFocus={handleFocus}
          onBlur={handleBlur}
          placeholder={placeholder}
          autoComplete="off"
        />
        <span className={`status-icon ${statusClass}`} title={statusTitle}>
          {statusIcon}
        </span>
        {dropdownVisible && (
          <div className="directory-list">
            {isLoading ? (
              <div className="directory-list-loading">
                <Skeleton width="60%" height={13} />
                <Skeleton width="45%" height={13} />
                <Skeleton width="55%" height={13} />
              </div>
            ) : suggestions.length === 0 ? (
              <div className="list-empty">No subdirectories</div>
            ) : (
              suggestions.map((entry, index) => (
                <button
                  key={entry.name}
                  className={`directory-entry${index === selectedIndex ? ' selected' : ''}`}
                  onMouseDown={(e) => {
                    e.preventDefault(); // Prevent input blur
                    handleSelect(entry);
                  }}
                  tabIndex={-1}
                >
                  <Folder size={14} className="entry-icon" />
                  <span className="entry-name">{entry.name}</span>
                  <ChevronRight size={14} className="entry-arrow" />
                </button>
              ))
            )}
          </div>
        )}
      </div>
    </div>
  );
}
