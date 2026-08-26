import { useState, useEffect, useRef, useCallback } from 'react';
import { api, ExpansionError } from '../api';
import { subscribeModels } from '../modelsPoller';
import type { ImageData, ModelEffort, ModelsResponse } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const NEW_CONVERSATION_DRAFT_KEY = 'phoenix-new-conversation-draft';

function effortSupportedByModel(models: ModelsResponse | null, modelId: string | null, effort: ModelEffort | null): boolean {
  if (!effort) return true;
  const capabilities = models?.models.find((model) => model.id === modelId)?.effort_capabilities;
  return capabilities?.support === 'supported' && capabilities.levels.includes(effort);
}

export function reconcileSubscribedModelSelection(
  modelsData: ModelsResponse,
  previousModel: string | null,
  previousEffort: ModelEffort | null,
): { selectedModel: string | null; selectedEffort: ModelEffort | null } {
  const selectedModel = modelsData.models.length === 0
    ? null
    : previousModel && modelsData.models.some((model) => model.id === previousModel)
      ? previousModel
      : modelsData.default;
  return {
    selectedModel,
    selectedEffort: effortSupportedByModel(modelsData, selectedModel, previousEffort)
      ? previousEffort
      : null,
  };
}

function readNewConversationDraft(): string {
  try {
    return localStorage.getItem(NEW_CONVERSATION_DRAFT_KEY) ?? '';
  } catch (error) {
    console.warn('Error reading new conversation draft from localStorage:', error);
    return '';
  }
}

function writeNewConversationDraft(value: string): void {
  try {
    if (value) {
      localStorage.setItem(NEW_CONVERSATION_DRAFT_KEY, value);
    } else {
      localStorage.removeItem(NEW_CONVERSATION_DRAFT_KEY);
    }
  } catch (error) {
    console.warn('Error saving new conversation draft to localStorage:', error);
  }
}

function clearNewConversationDraft(): void {
  try {
    localStorage.removeItem(NEW_CONVERSATION_DRAFT_KEY);
  } catch (error) {
    console.warn('Error clearing new conversation draft from localStorage:', error);
  }
}

export function useCreateConversation(navigate: (path: string) => void) {
  const [homeDir, setHomeDir] = useState<string>('');
  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '');
  const [dirStatus, setDirStatus] = useState<DirStatus>(() =>
    localStorage.getItem(LAST_CWD_KEY) ? 'exists' : 'checking'
  );
  const [isGitDir, setIsGitDir] = useState<boolean | null>(null);
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [selectedEffort, setSelectedEffort] = useState<ModelEffort | null>(null);
  const [showAllModels, setShowAllModels] = useState(false);
  const [draft, setDraft] = useState(readNewConversationDraft);
  const [images, setImages] = useState<ImageData[]>([]);
  const [files, setFiles] = useState<File[]>([]);
  const [isDragOver, setIsDragOver] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');
  const selectedModelRef = useRef(selectedModel);
  const selectedEffortRef = useRef(selectedEffort);
  const selectModel = useCallback((model: string | null) => {
    selectedModelRef.current = model;
    setSelectedModel(model);
  }, []);
  const selectEffort = useCallback((effort: ModelEffort | null) => {
    selectedEffortRef.current = effort;
    setSelectedEffort(effort);
  }, []);

  useEffect(() => {
    selectedModelRef.current = selectedModel;
  }, [selectedModel]);
  useEffect(() => {
    selectedEffortRef.current = selectedEffort;
  }, [selectedEffort]);

  useEffect(() => {
    const unsub = subscribeModels(modelsData => {
      setModels(modelsData);
      const next = reconcileSubscribedModelSelection(
        modelsData,
        selectedModelRef.current,
        selectedEffortRef.current,
      );
      selectModel(next.selectedModel);
      selectEffort(next.selectedEffort);
    });
    api.getEnv().then(env => {
      setHomeDir(env.home_dir);
      if (!localStorage.getItem(LAST_CWD_KEY)) {
        setCwd(env.home_dir);
      }
    }).catch(console.error);
    return () => { unsub(); };
  }, [selectEffort, selectModel]);

  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);
  useEffect(() => {
    if (!effortSupportedByModel(models, selectedModel, selectedEffort)) {
      selectEffort(null);
    }
  }, [models, selectedEffort, selectedModel, selectEffort]);
  useEffect(() => { writeNewConversationDraft(draft); }, [draft]);

  const genericFilesEnabled = false;
  const hasMessageContent = draft.trim().length > 0 || images.length > 0 || files.length > 0;
  const canSend = hasMessageContent
    && !creating
    && dirStatus !== 'invalid'
    && dirStatus !== 'checking';

  const addImages = async (files: File[]) => {
    try {
      const newImages = await processImageFiles(files);
      setImages(prev => [...prev, ...newImages]);
    } catch (err) {
      console.error('Error processing images:', err);
    }
  };

  const removeImage = (index: number) => {
    setImages(prev => prev.filter((_, idx) => idx !== index));
  };

  const addFiles = async (dropped: File[]) => {
    const unsupportedImage = dropped.find(file => file.type.startsWith('image/') && !SUPPORTED_IMAGE_TYPES.includes(file.type));
    if (unsupportedImage) {
      setError(`${unsupportedImage.name} is not a supported image attachment type.`);
      return;
    }
    const imageFiles = dropped.filter(file => SUPPORTED_IMAGE_TYPES.includes(file.type));
    const genericFiles = dropped.filter(file => !SUPPORTED_IMAGE_TYPES.includes(file.type));
    if (imageFiles.length > 0) await addImages(imageFiles);
    if (genericFiles.length > 0) {
      setError('File attachments are not available for this conversation flow yet.');
    }
  };

  const removeFile = (index: number) => {
    setFiles(prev => prev.filter((_, idx) => idx !== index));
  };

  const handleVoiceFinal = (text: string) => {
    if (!text) return;
    setInterimText('');
    const baseDraft = draftBeforeVoiceRef.current || draft;
    const newDraft = baseDraft.trim() ? baseDraft.trimEnd() + ' ' + text : text;
    setDraft(newDraft);
    draftBeforeVoiceRef.current = newDraft;
  };

  const handleVoiceInterim = (text: string) => {
    if (!interimText && text) draftBeforeVoiceRef.current = draft;
    setInterimText(text);
  };

  const updateDraft = (value: string) => {
    setDraft(value);
    if (interimText) {
      setInterimText('');
      draftBeforeVoiceRef.current = '';
    }
  };

  const textareaValue = interimText
    ? (draft.trim() ? draft.trimEnd() + ' ' + interimText : interimText)
    : draft;

  const handleSend = async () => {
    const trimmed = draft.trim();
    if (!trimmed && images.length === 0 && files.length === 0) return;
    if (creating || dirStatus === 'invalid' || dirStatus === 'checking') return;

    setError(null);
    setCreating(true);

    try {
      if (dirStatus === 'will-create') {
        const mkdirResult = await api.mkdir(cwd.trim());
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Failed to create directory');
          setCreating(false);
          return;
        }
      }

      const messageId = generateUUID();
      const clientConversationId = generateUUID();
      const trimmedCwd = cwd.trim();
      if (files.length > 0) {
        setError('File attachments are not available for this conversation flow yet.');
        setCreating(false);
        return;
      }
      if (!selectedModel) {
        setError('Pick a model before creating the conversation.');
        setCreating(false);
        return;
      }
      const response = await api.createProductConversation({
        cwd: trimmedCwd,
        objective: trimmed,
        message_id: messageId,
        conversation_id: clientConversationId,
        model: selectedModel,
        effort: selectedEffort,
        images,
        settings: {},
      });
      setDraft('');
      setImages([]);
      setFiles([]);
      clearNewConversationDraft();
      navigate(response.canonical_route);
    } catch (err) {
      setCreating(false);
      if (err instanceof ExpansionError) {
        throw err;
      }
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
    }
  };

  return {
    homeDir,
    cwd,
    setCwd,
    dirStatus,
    setDirStatus,
    isGitDir,
    setIsGitDir,
    models,
    selectedModel,
    setSelectedModel: selectModel,
    selectedEffort,
    setSelectedEffort: selectEffort,
    showAllModels,
    setShowAllModels,
    draft,
    setDraft,
    images,
    files,
    isDragOver,
    setIsDragOver,
    error,
    creating,
    canSend,
    genericFilesEnabled,
    addImages,
    removeImage,
    addFiles,
    removeFile,
    voiceSupported,
    handleVoiceFinal,
    handleVoiceInterim,
    textareaValue,
    updateDraft,
    handleSend,
  };
}
