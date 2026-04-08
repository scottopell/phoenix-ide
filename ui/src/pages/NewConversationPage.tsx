import { useEffect, useRef, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { ImageAttachments } from '../components/ImageAttachments';
import { ConversationSettings } from '../components/ConversationSettings';
import { DIR_STATUS_CONFIG, SettingsFields } from '../components/SettingsFields';
import { VoiceRecorder } from '../components/VoiceInput/VoiceRecorder';
import { SUPPORTED_IMAGE_TYPES } from '../utils/images';
import { useCreateConversation } from '../hooks/useCreateConversation';
import { useState } from 'react';

interface NewConversationPageProps {
  desktopMode?: boolean;
}

export function NewConversationPage({ desktopMode }: NewConversationPageProps = {}) {
  const navigate = useNavigate();
  const conv = useCreateConversation(navigate);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [showSettings, setShowSettings] = useState(false);

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
    if (files.length > 0) await conv.addImages(files);
    e.target.value = '';
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      conv.handleSend();
    }
  };

  const { icon: dirStatusIcon, class: dirStatusClass } = DIR_STATUS_CONFIG[conv.dirStatus];
  const cwdDisplay = (conv.homeDir && conv.cwd.trim().startsWith(conv.homeDir))
    ? '~/' + conv.cwd.trim().slice(conv.homeDir.length).replace(/^\//, '')
    : conv.cwd.trim() || '~/';
  const modelDisplay = conv.models?.models.find(m => m.id === conv.selectedModel)?.id.replace(/-sonnet|-opus/g, '') || '...';
  const buttonText = conv.creating ? (conv.dirStatus === 'will-create' ? 'Creating folder...' : 'Creating...') : 'Send';

  return (
    <div className="new-conv-page">
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
            mode={conv.mode}
            setMode={conv.setMode}
            branches={conv.branches}
            currentBranch={conv.currentBranch}
            baseBranch={conv.baseBranch}
            setBaseBranch={conv.setBaseBranch}
          />

          {/* Main input */}
          <ImageAttachments images={conv.images} onRemove={conv.removeImage} />
          <textarea
            ref={textareaRef}
            className="new-conv-textarea"
            placeholder="What would you like to work on?"
            rows={3}
            value={conv.textareaValue}
            onChange={(e) => conv.updateDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            disabled={conv.creating}
          />

          {/* Actions row: settings chips + send */}
          <div className="new-conv-actions">
            <div className="new-conv-chips">
              <button className="new-conv-chip" title={conv.cwd} onClick={() => setShowSettings(!showSettings)}>
                <span className={`chip-status ${dirStatusClass}`}>{dirStatusIcon}</span>
                {cwdDisplay}
              </button>
              <button className="new-conv-chip" onClick={() => setShowSettings(!showSettings)}>
                {modelDisplay}
              </button>
            </div>
            <div className="new-conv-send-group">
              <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={conv.creating}>+</button>
              {conv.voiceSupported && <VoiceRecorder onSpeech={conv.handleVoiceFinal} onInterim={conv.handleVoiceInterim} disabled={conv.creating} />}
              <button className="new-conv-send" onClick={() => conv.handleSend()} disabled={!conv.canSend}>{buttonText}</button>
            </div>
          </div>

          {/* Expanded settings (bare SettingsFields only — banner/error/recent/preview already shown above) */}
          {showSettings && (
            <div className="new-conv-settings-expanded">
              <SettingsFields
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
              />
            </div>
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
            mode={conv.mode}
            setMode={conv.setMode}
            branches={conv.branches}
            currentBranch={conv.currentBranch}
            baseBranch={conv.baseBranch}
            setBaseBranch={conv.setBaseBranch}
          />
        </div>
      </main>

      {/* Mobile: bottom-anchored input */}
      <div className="new-conv-bottom-input mobile-only">
        <ImageAttachments images={conv.images} onRemove={conv.removeImage} />
        <textarea
          className="new-conv-textarea-mobile"
          placeholder="What would you like to work on?"
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
    </div>
  );
}
