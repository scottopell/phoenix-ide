import { lazy, Suspense, useEffect, useRef, useState, KeyboardEvent, ClipboardEvent, ChangeEvent, DragEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { ImageAttachments } from '../components/ImageAttachments';
import { ConversationSettings } from '../components/ConversationSettings';
import { VoiceRecorder } from '../components/VoiceInput/VoiceRecorder';
import { PaneDivider } from '../components/PaneDivider';
import { SUPPORTED_IMAGE_TYPES } from '../utils/images';
import { useCreateConversation } from '../hooks/useCreateConversation';
import { useResizablePane } from '../hooks/useResizablePane';
import { useIsDesktop } from '../hooks/useMediaQuery';

// Lazy: xterm + addon are a non-trivial bundle slice. Deferred behind the
// `everExpanded` gate below — the dynamic import only fires once the user
// expands the global terminal pane, at which point we keep TerminalPanel
// mounted so the WebSocket and shell state survive subsequent collapse/expand.
const TerminalPanel = lazy(() =>
  import('../components/TerminalPanel').then((m) => ({ default: m.TerminalPanel })),
);

const GLOBAL_TERMINAL_COLLAPSED_PX = 32;

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function NewConversationFileChips({ files, onRemove }: { files: File[]; onRemove: (index: number) => void }) {
  if (files.length === 0) return null;
  return (
    <div className="file-attachments" aria-label="Attached files">
      {files.map((file, index) => (
        <div key={`${file.name}-${file.size}-${index}`} className="file-attachment-chip">
          <span className="file-attachment-icon">📎</span>
          <span className="file-attachment-name" title={file.name}>{file.name}</span>
          <span className="file-attachment-size">{formatBytes(file.size)}</span>
          <button
            type="button"
            className="file-attachment-remove"
            onClick={() => onRemove(index)}
            title="Remove attachment"
            aria-label={`Remove attachment ${file.name}`}
          >
            x
          </button>
        </div>
      ))}
    </div>
  );
}

interface NewConversationPageProps {
  desktopMode?: boolean;
}

export function NewConversationPage({ desktopMode }: NewConversationPageProps = {}) {
  const navigate = useNavigate();
  const conv = useCreateConversation(navigate);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Real-breakpoint gate for the global terminal. CSS `display:none` would
  // hide it on mobile but React effects (lazy import, WebSocket open,
  // xterm allocation) still run — pulling the singleton tmux/PTY into
  // existence even on devices where the user can't see or use it.
  const isDesktop = useIsDesktop();

  // Global terminal split-pane. Persistence key is distinct from the
  // per-conversation terminal so the two heights don't fight. Default
  // collapsed: opt-in surface, mirrors ConversationPage's default.
  const terminalPane = useResizablePane({
    key: 'global-terminal-height',
    min: GLOBAL_TERMINAL_COLLAPSED_PX,
    max: () => Math.min(800, Math.floor(window.innerHeight * 0.75)),
    defaultSize: 300,
    collapseThreshold: 60,
    defaultCollapsed: true,
  });

  // Defer mounting TerminalPanel until first expand. The lazy chunk import,
  // WebSocket open, and xterm allocation all happen inside TerminalPanel's
  // mount effect; without this gate, a user who never expands the pane
  // still pays for all of it on every visit to /new. Once mounted, the
  // panel stays mounted so collapse/expand preserves shell state.
  const [everExpanded, setEverExpanded] = useState(!terminalPane.collapsed);

  // Auto-resize textarea
  useEffect(() => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
    }
  }, [conv.draft]);

  // Focus textarea on mount
  useEffect(() => { textareaRef.current?.focus(); }, []);

  const handlePaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (const item of items) {
      if (item.type.startsWith('image/')) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length > 0) {
      e.preventDefault();
      await conv.addImages(imageFiles);
    }
  };

  const handleFileChange = async (e: ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0) await conv.addFiles(files);
    e.target.value = '';
  };

  const handleDrop = async (e: DragEvent) => {
    if (!Array.from(e.dataTransfer.types).includes('Files')) return;
    e.preventDefault();
    conv.setIsDragOver(false);
    const dropped = Array.from(e.dataTransfer.files);
    if (dropped.length > 0) await conv.addFiles(dropped);
  };

  const handleDragEnter = (e: DragEvent) => {
    if (Array.from(e.dataTransfer.types).includes('Files')) {
      e.preventDefault();
      conv.setIsDragOver(true);
    }
  };

  const handleDragOver = (e: DragEvent) => {
    if (Array.from(e.dataTransfer.types).includes('Files')) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
    }
  };

  const handleDragLeave = (e: DragEvent) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
      conv.setIsDragOver(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      conv.handleSend();
    }
  };

  const buttonText = conv.creating ? (conv.dirStatus === 'will-create' ? 'Creating folder...' : 'Creating...') : 'Send';
  // Until an LLM is configured, the only meaningful action is the sign-in
  // CTA inside ConversationSettings. Hide the message composer + actions so
  // the user isn't tempted to type into a draft that can't be sent.
  const llmReady = conv.models === null || conv.models.llm_configured;

  const inputPlaceholder = conv.workflow.kind === 'planFromTask'
    ? 'Optional notes for this task…'
    : 'What would you like to work on?';

  return (
    <div
      className={`new-conv-page${conv.isDragOver ? ' input-area--drag-over' : ''}`}
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      <input
        ref={fileInputRef}
        type="file"
        accept={SUPPORTED_IMAGE_TYPES.join(',')}
        multiple
        onChange={handleFileChange}
        style={{ display: 'none' }}
      />

      {!desktopMode && (
        <header className="new-conv-header-minimal">
          <button className="back-link" onClick={() => navigate('/')}>← Back</button>
        </header>
      )}

      <main className="new-conv-main">
        {/* Desktop: workbench card */}
        <div className="new-conv-card desktop-only">
          <ConversationSettings
            cwd={conv.cwd}
            setCwd={conv.setCwd}
            dirStatus={conv.dirStatus}
            onDirStatusChange={conv.setDirStatus}
            onGitStatusChange={conv.setIsGitDir}
            selectedModel={conv.selectedModel}
            setSelectedModel={conv.setSelectedModel}
            models={conv.models}
            showAllModels={conv.showAllModels}
            setShowAllModels={conv.setShowAllModels}
            recentDirs={conv.recentDirs}
            isGitDir={conv.isGitDir}
            error={conv.error}
            workflow={conv.workflow}
            setWorkflow={conv.setWorkflow}
            tasks={conv.tasks}
            branches={conv.branches}
            currentBranch={conv.currentBranch}
            gitMetadataLoading={conv.gitMetadataLoading}
            branchSearch={conv.branchSearch}
            setBranchSearch={conv.setBranchSearch}
            branchSearchLoading={conv.branchSearchLoading}
          />

          {/* Main input — hidden until an LLM is configured */}
          {llmReady && (
            <>
              {conv.isDragOver && <div className="input-drop-affordance">Drop files to attach</div>}
              <ImageAttachments images={conv.images} onRemove={conv.removeImage} />
              <NewConversationFileChips files={conv.files} onRemove={conv.removeFile} />
              <textarea
                ref={textareaRef}
                className="new-conv-textarea"
                placeholder={inputPlaceholder}
                rows={3}
                value={conv.textareaValue}
                onChange={(e) => conv.updateDraft(e.target.value)}
                onKeyDown={handleKeyDown}
                onPaste={handlePaste}
                disabled={conv.creating}
              />

              {/* Actions row: send group */}
              <div className="new-conv-actions">
                <div className="new-conv-send-group">
                  <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={conv.creating}>+</button>
                  {conv.voiceSupported && <VoiceRecorder onSpeech={conv.handleVoiceFinal} onInterim={conv.handleVoiceInterim} disabled={conv.creating} />}
                  <button className="new-conv-send" onClick={() => conv.handleSend()} disabled={!conv.canSend}>{buttonText}</button>
                </div>
              </div>
            </>
          )}
        </div>

        {/* Mobile: keep existing layout */}
        <div className="new-conv-content mobile-only">
          <ConversationSettings
            cwd={conv.cwd}
            setCwd={conv.setCwd}
            dirStatus={conv.dirStatus}
            onDirStatusChange={conv.setDirStatus}
            onGitStatusChange={conv.setIsGitDir}
            selectedModel={conv.selectedModel}
            setSelectedModel={conv.setSelectedModel}
            models={conv.models}
            showAllModels={conv.showAllModels}
            setShowAllModels={conv.setShowAllModels}
            isGitDir={conv.isGitDir}
            error={conv.error}
            workflow={conv.workflow}
            setWorkflow={conv.setWorkflow}
            tasks={conv.tasks}
            branches={conv.branches}
            currentBranch={conv.currentBranch}
            gitMetadataLoading={conv.gitMetadataLoading}
            branchSearch={conv.branchSearch}
            setBranchSearch={conv.setBranchSearch}
            branchSearchLoading={conv.branchSearchLoading}
          />
        </div>
      </main>

      {/* Global terminal — singleton WorkScope::Global session,
          survives navigation and conversation lifecycles. The divider is
          always rendered on desktop so the user has an affordance to
          expand; TerminalPanel itself only mounts on first expand
          (see `everExpanded` above). Mobile renders nothing — no PTY,
          no WebSocket, no xterm bundle fetch. */}
      {isDesktop && (
        <>
          <PaneDivider
            orientation="horizontal"
            title="Drag to resize • Double-click to collapse/expand"
            onPointerDown={(e) => {
              setEverExpanded(true);
              terminalPane.startDrag(e, 'y', true);
            }}
            onDoubleClick={() => {
              if (terminalPane.collapsed) {
                setEverExpanded(true);
                terminalPane.expandFromCollapsed();
              } else {
                terminalPane.setCollapsed(true);
              }
            }}
          />
          {everExpanded && (
            <Suspense fallback={null}>
              <TerminalPanel
                scope={{ kind: 'global' }}
                height={terminalPane.collapsed ? GLOBAL_TERMINAL_COLLAPSED_PX : terminalPane.size}
                collapsed={terminalPane.collapsed}
                onExpand={() => {
                  setEverExpanded(true);
                  terminalPane.expandFromCollapsed();
                }}
                onCollapse={() => terminalPane.setCollapsed(true)}
              />
            </Suspense>
          )}
        </>
      )}

      {/* Mobile: bottom-anchored input — hidden until an LLM is configured */}
      {llmReady && (
        <div className="new-conv-bottom-input mobile-only">
          <ImageAttachments images={conv.images} onRemove={conv.removeImage} />
          <NewConversationFileChips files={conv.files} onRemove={conv.removeFile} />
          <textarea
            className="new-conv-textarea-mobile"
            placeholder={inputPlaceholder}
            rows={2}
            value={conv.textareaValue}
            onChange={(e) => conv.updateDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            disabled={conv.creating}
          />
          <div className="new-conv-input-row">
            <div className="new-conv-input-left">
              <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={conv.creating}>+</button>
              {conv.voiceSupported && <VoiceRecorder onSpeech={conv.handleVoiceFinal} onInterim={conv.handleVoiceInterim} disabled={conv.creating} />}
            </div>
            <button className="new-conv-send" onClick={() => conv.handleSend()} disabled={!conv.canSend}>{buttonText}</button>
          </div>
        </div>
      )}
    </div>
  );
}
