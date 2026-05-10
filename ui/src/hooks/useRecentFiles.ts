import { useState, useCallback } from 'react';

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
  // In-render reset on conversationId change. The previous `useEffect`-based
  // reload committed the prior conversation's recent-files for one frame on
  // returning navigation (visit A → visit B → return to A) before the effect
  // ran. Re-reading storage during render keeps the list in lockstep with the
  // current conversation prop without a commit gap.
  const [files, setFiles] = useState<RecentFile[]>(() => loadRecent(conversationId));
  const [trackedConversationId, setTrackedConversationId] = useState<string | undefined>(conversationId);

  let currentFiles = files;
  if (trackedConversationId !== conversationId) {
    setTrackedConversationId(conversationId);
    currentFiles = loadRecent(conversationId);
    setFiles(currentFiles);
  }

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

  return { recentFiles: currentFiles, addRecentFile };
}
