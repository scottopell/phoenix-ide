/**
 * FileBrowserOverlay — Mobile modal overlay hosting FileTree
 * REQ-FE-010
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { GitBranch, X } from 'lucide-react';
import { FileTree } from './FileTree';
import { checkoutLabel } from './gitStatusPresentation';
import { api, type ConversationGitStatusResponse } from '../../api';
import { useViewerSlotCommands } from '../../contexts/ViewerSlotContext';
import './FileBrowserOverlay.css';

interface Props {
  isOpen: boolean;
  rootPath: string;
  conversationId: string;
  onClose: () => void;
  onFileSelect: (filePath: string, rootDir: string) => void;
  canOpenWorkspaceDiff?: boolean | undefined;
}

function mobileGitSummary(status: ConversationGitStatusResponse | null): string {
  if (!status) return 'Checking Git…';
  if (status.kind === 'non_git') return 'Not a Git workspace';
  if (status.kind === 'unavailable') return 'Git unavailable';
  return `${checkoutLabel(status.checkout_status)} · ${status.counts.changed_paths === 0 ? 'clean' : `${status.counts.changed_paths} changes`}`;
}

export function FileBrowserOverlay({ isOpen, rootPath, conversationId, onClose, onFileSelect, canOpenWorkspaceDiff = false }: Props) {
  const [gitStatus, setGitStatus] = useState<ConversationGitStatusResponse | null>(null);
  const requestRef = useRef<AbortController | null>(null);
  const { openDiffFullscreen } = useViewerSlotCommands();
  const loadGitStatus = useCallback(async () => {
    requestRef.current?.abort();
    const controller = new AbortController();
    requestRef.current = controller;
    try {
      const status = await api.getConversationGitStatus(conversationId, controller.signal);
      if (!controller.signal.aborted) setGitStatus(status);
    } catch (error) {
      if (!controller.signal.aborted) setGitStatus({
        kind: 'unavailable',
        reason: error instanceof Error ? error.message : 'Git status is unavailable.',
      });
    }
  }, [conversationId]);

  useEffect(() => {
    if (!isOpen) return;
    setGitStatus(null);
    void loadGitStatus();
    return () => requestRef.current?.abort();
  }, [isOpen, rootPath, loadGitStatus]);

  if (!isOpen) return null;

  const displayPath = rootPath.length > 40
    ? '.../' + rootPath.split('/').slice(-2).join('/')
    : rootPath;

  return (
    <div className="file-browser-overlay" onClick={onClose}>
      <div className="file-browser-container" onClick={e => e.stopPropagation()}>
        <div className="file-browser-header">
          <div className="file-browser-heading">
            <div className="file-browser-path" title={rootPath}>{displayPath}</div>
            {canOpenWorkspaceDiff ? <button
              type="button"
              className="file-browser-git-summary"
              onClick={() => {
                onClose();
                openDiffFullscreen('workspace');
              }}
              aria-label={`${mobileGitSummary(gitStatus)}. Open Workspace Diff`}
            >
              <GitBranch size={14} aria-hidden="true" />
              <span>{mobileGitSummary(gitStatus)}</span>
            </button> : (
              <div className="file-browser-git-summary" aria-label={mobileGitSummary(gitStatus)}>
                <GitBranch size={14} aria-hidden="true" />
                <span>{mobileGitSummary(gitStatus)}</span>
              </div>
            )}
          </div>
          <button className="file-browser-btn" onClick={onClose} aria-label="Close">
            <X size={20} />
          </button>
        </div>
        <div className="file-browser-content">
          <FileTree
            key={`${conversationId}\0${rootPath}`}
            rootPath={rootPath}
            onFileSelect={onFileSelect}
            conversationId={conversationId}
            gitStatus={gitStatus}
            onRefreshTick={loadGitStatus}
          />
        </div>
      </div>
    </div>
  );
}
