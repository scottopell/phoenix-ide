import { useState, useCallback, useEffect } from 'react';

export interface RecentFile {
  path: string;
  name: string;
  openedAt: number;
}

const MAX_RECENT = 5;

function storageKey(conversationId: string): string {
  return `phoenix:recent-files:${conversationId}`;
}

function loadRecent(conversationId: string | undefined): RecentFile[] {
  if (!conversationId) return [];
  try {
    const raw = localStorage.getItem(storageKey(conversationId));
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveRecent(conversationId: string, files: RecentFile[]) {
  localStorage.setItem(storageKey(conversationId), JSON.stringify(files));
}

export function useRecentFiles(conversationId: string | undefined) {
  const [files, setFiles] = useState<RecentFile[]>(() => loadRecent(conversationId));

  // Reload when conversation changes
  useEffect(() => {
    setFiles(loadRecent(conversationId));
  }, [conversationId]);

  const addRecentFile = useCallback((path: string) => {
    if (!conversationId) return;
    setFiles(prev => {
      const name = path.split('/').pop() || path;
      const filtered = prev.filter(f => f.path !== path);
      const updated = [{ path, name, openedAt: Date.now() }, ...filtered].slice(0, MAX_RECENT);
      saveRecent(conversationId, updated);
      return updated;
    });
  }, [conversationId]);

  return { recentFiles: files, addRecentFile };
}
