import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { api, ExpansionError } from '../api';
import { subscribeModels } from '../modelsPoller';
import type { CreateProductConversationRequest, ImageData, ModelEffort, ModelsResponse, RecentManagementRootSuggestion } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const NEW_CONVERSATION_DRAFT_KEY = 'phoenix-new-conversation-draft';
const REPLAY_CREATE_REQUEST_KEY = 'phoenix-replay-product-create-request';

export function beginNewProductConversationIntent(): void {
  try {
    localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY);
    localStorage.removeItem('phoenix-pending-product-create-request');
  } catch { /* best effort */ }
}

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
  const requestId = useMemo(() => {
    try {
      const replay = localStorage.getItem(REPLAY_CREATE_REQUEST_KEY);
      if (replay) return replay;
    } catch { /* use a fresh in-memory identity */ }
    return generateUUID();
  }, []);
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
  const [recentManagementRootSuggestions, setRecentManagementRootSuggestions] = useState<RecentManagementRootSuggestion[]>([]);

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
    api.listRecentManagementRootSuggestions()
      .then((response) => setRecentManagementRootSuggestions(response.suggestions))
      .catch(console.error);
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
      const trimmedCwd = cwd.trim();
      if (dirStatus === 'will-create') {
        const mkdirResult = await api.mkdir(trimmedCwd);
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Failed to create directory');
          setCreating(false);
          return;
        }
      }

      try { localStorage.setItem(REPLAY_CREATE_REQUEST_KEY, requestId); } catch { /* in-memory identity remains stable */ }
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
      const createRequest: CreateProductConversationRequest = {
        request_id: requestId,
        cwd: trimmedCwd,
        objective: trimmed,
        model: selectedModel,
        effort: selectedEffort,
        ...(images.length > 0 ? { images } : {}),
      };
      const response = await api.createProductConversation(createRequest);
      setDraft('');
      setImages([]);
      setFiles([]);
      clearNewConversationDraft();
      try { localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY); } catch { /* best effort after success */ }
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
    recentManagementRootSuggestions,
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
