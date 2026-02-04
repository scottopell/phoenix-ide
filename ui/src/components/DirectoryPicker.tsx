import { useState, useEffect, useCallback } from 'react';
import { enhancedApi } from '../enhancedApi';

interface DirectoryEntry {
  name: string;
  is_dir: boolean;
}

interface DirectoryPickerProps {
  value: string;
  onChange: (path: string) => void;
}

type PathStatus = 
  | { type: 'exists' }
  | { type: 'will_create'; parent: string }
  | { type: 'invalid'; error: string }
  | { type: 'checking' };

export function DirectoryPicker({ value, onChange }: DirectoryPickerProps) {
  const [entries, setEntries] = useState<DirectoryEntry[]>([]);
  const [listError, setListError] = useState<string | null>(null);
  const [pathStatus, setPathStatus] = useState<PathStatus>({ type: 'checking' });
  
  // Parse path into segments for breadcrumbs
  const pathSegments = value.split('/').filter(Boolean);
  
  // Find the deepest existing directory for listing
  const [listingPath, setListingPath] = useState(value);

  // Check path status and load directory listing
  useEffect(() => {
    const checkPath = async () => {
      setPathStatus({ type: 'checking' });
      
      const validation = await enhancedApi.validateCwd(value);
      
      if (validation.valid) {
        setPathStatus({ type: 'exists' });
        setListingPath(value);
        return;
      }
      
      // Path doesn't exist - check if parent exists (would create)
      const parentPath = value.substring(0, value.lastIndexOf('/')) || '/';
      const parentValidation = await enhancedApi.validateCwd(parentPath);
      
      if (parentValidation.valid) {
        setPathStatus({ type: 'will_create', parent: parentPath });
        setListingPath(parentPath);
      } else {
        setPathStatus({ type: 'invalid', error: 'Parent directory does not exist' });
        // Try to find deepest existing ancestor
        let ancestor = parentPath;
        while (ancestor !== '/') {
          const ancestorParent = ancestor.substring(0, ancestor.lastIndexOf('/')) || '/';
          const ancestorCheck = await enhancedApi.validateCwd(ancestorParent);
          if (ancestorCheck.valid) {
            setListingPath(ancestorParent);
            return;
          }
          ancestor = ancestorParent;
        }
        setListingPath('/');
      }
    };
    
    checkPath();
  }, [value]);

  // Load directory entries when listing path changes
  useEffect(() => {
    const loadEntries = async () => {
      try {
        const resp = await enhancedApi.listDirectory(listingPath);
        setEntries(resp.entries.filter(e => e.is_dir));
        setListError(null);
      } catch (err) {
        setListError(err instanceof Error ? err.message : 'Failed to list directory');
        setEntries([]);
      }
    };
    
    loadEntries();
  }, [listingPath]);

  const handleBreadcrumbClick = useCallback((index: number) => {
    const newPath = '/' + pathSegments.slice(0, index + 1).join('/');
    onChange(newPath);
  }, [pathSegments, onChange]);

  const handleEntryClick = useCallback((entry: DirectoryEntry) => {
    const newPath = listingPath === '/' 
      ? `/${entry.name}` 
      : `${listingPath}/${entry.name}`;
    onChange(newPath);
  }, [listingPath, onChange]);

  const handleGoUp = useCallback(() => {
    const parentPath = value.substring(0, value.lastIndexOf('/')) || '/';
    onChange(parentPath);
  }, [value, onChange]);

  const statusIcon = () => {
    switch (pathStatus.type) {
      case 'checking':
        return <span className="status-icon checking">⋯</span>;
      case 'exists':
        return <span className="status-icon exists">✓</span>;
      case 'will_create':
        return <span className="status-icon will-create">+</span>;
      case 'invalid':
        return <span className="status-icon invalid">✗</span>;
    }
  };

  return (
    <div className="directory-picker">
      {/* Path input with status */}
      <div className="path-input-container">
        <input
          type="text"
          className={`path-input ${pathStatus.type}`}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="/home/exedev/project"
        />
        {statusIcon()}
      </div>

      {/* Breadcrumb navigation with Up button */}
      <div className="picker-nav">
        <button 
          className="picker-up-btn"
          onClick={handleGoUp}
          disabled={value === '/'}
          title="Go up"
        >
          ↑
        </button>
        <div className="picker-breadcrumbs">
          <button 
            className="breadcrumb-btn root" 
            onClick={() => onChange('/')}
          >
            /
          </button>
          {pathSegments.map((segment, i) => (
            <span key={i} className="breadcrumb-segment">
              <span className="breadcrumb-sep">/</span>
              <button 
                className={`breadcrumb-btn ${i === pathSegments.length - 1 ? 'active' : ''}`}
                onClick={() => handleBreadcrumbClick(i)}
              >
                {segment}
              </button>
            </span>
          ))}
        </div>
      </div>

      {/* Directory listing */}
      <div className="directory-list">
        {listError ? (
          <div className="list-error">{listError}</div>
        ) : entries.length === 0 ? (
          <div className="list-empty">No subdirectories</div>
        ) : (
          entries.map((entry) => (
            <button
              key={entry.name}
              className="directory-entry"
              onClick={() => handleEntryClick(entry)}
            >
              <span className="entry-icon">📁</span>
              <span className="entry-name">{entry.name}</span>
              <span className="entry-arrow">›</span>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
