import { useState, useEffect, useRef, useCallback } from 'react';
import { api, ApiResponseError, ExpansionError } from '../api';
import { subscribeModels } from '../modelsPoller';
import type { CreateProductConversationRequest, ImageData, ModelEffort, ModelsResponse, ProductConversationCreationObservation, RecentManagementRootSuggestion } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const NEW_CONVERSATION_DRAFT_KEY = 'phoenix-new-conversation-draft';
const REPLAY_CREATE_REQUEST_KEY = 'phoenix-replay-product-create-request';
const REPLAY_CREATE_INTENT_KEY = 'phoenix-replay-product-create-intent';
const RECOVERED_LLM_LANGUAGE_KEY = 'phoenix-create-llm-language';
const PENDING_CREATE_REQUEST_KEY = 'phoenix-pending-product-create-request';
const CREATION_OBSERVATION_INTERVAL_MS = 2000;

type PendingCreationRequest =
  | { readonly version: 2; readonly kind: 'uncertain'; readonly request: CreateProductConversationRequest }
  | { readonly version: 2; readonly kind: 'accepted'; readonly requestId: string; readonly productConversationId: string };

function creationRequestId(pending: PendingCreationRequest): string {
  return pending.kind === 'uncertain' ? pending.request.request_id : pending.requestId;
}

function isCreateRequest(value: unknown): value is CreateProductConversationRequest {
  if (!value || typeof value !== 'object') return false;
  const request = value as Record<string, unknown>;
  const efforts: readonly ModelEffort[] = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
  return typeof request.request_id === 'string' && request.request_id.length > 0
    && typeof request.cwd === 'string'
    && typeof request.objective === 'string'
    && typeof request.model === 'string' && request.model.length > 0
    && (request.effort == null || efforts.some((effort) => effort === request.effort))
    && (request.llm_language == null || typeof request.llm_language === 'string')
    && (request.images === undefined || (Array.isArray(request.images) && request.images.every((image: unknown) =>
      image !== null && typeof image === 'object'
      && 'data' in image && typeof image.data === 'string'
      && 'media_type' in image && typeof image.media_type === 'string')));
}

function readPendingCreationRequest(): PendingCreationRequest | null {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(PENDING_CREATE_REQUEST_KEY) ?? 'null');
    if (stored && typeof stored === 'object'
      && 'version' in stored && stored.version === 2 && 'kind' in stored) {
      if (stored.kind === 'uncertain' && 'request' in stored && isCreateRequest(stored.request)) {
        return { version: 2, kind: 'uncertain', request: stored.request };
      }
      if (stored.kind === 'accepted'
        && 'requestId' in stored && typeof stored.requestId === 'string' && stored.requestId
        && 'productConversationId' in stored && typeof stored.productConversationId === 'string' && stored.productConversationId) {
        return { version: 2, kind: 'accepted', requestId: stored.requestId, productConversationId: stored.productConversationId };
      }
    }
  } catch { /* invalid or unavailable storage has no resumable request */ }
  try { localStorage.removeItem(PENDING_CREATE_REQUEST_KEY); } catch { /* best effort */ }
  return null;
}

function storePendingCreationRequest(request: PendingCreationRequest | null): void {
  try {
    if (request) localStorage.setItem(PENDING_CREATE_REQUEST_KEY, JSON.stringify(request));
    else localStorage.removeItem(PENDING_CREATE_REQUEST_KEY);
  } catch { /* the mounted flow retains its immutable identity */ }
}

export function beginNewProductConversationIntent(): void {
  try {
    localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY);
    localStorage.removeItem(REPLAY_CREATE_INTENT_KEY);
    localStorage.removeItem(RECOVERED_LLM_LANGUAGE_KEY);
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
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);
  const [requestId, setRequestId] = useState(() => {
    const pending = readPendingCreationRequest();
    if (pending) return creationRequestId(pending);
    try {
      const replay = localStorage.getItem(REPLAY_CREATE_REQUEST_KEY);
      if (replay) return replay;
    } catch { /* use a fresh in-memory identity */ }
    return generateUUID();
  });
  const [recoveredLlmLanguage, setRecoveredLlmLanguage] = useState<string | null>(() => {
    try { return localStorage.getItem(RECOVERED_LLM_LANGUAGE_KEY); } catch { return null; }
  });
  const replayIntentRef = useRef<string | null>((() => {
    try { return localStorage.getItem(REPLAY_CREATE_INTENT_KEY); } catch { return null; }
  })());
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
  const [observedRequest, setObservedRequest] = useState<PendingCreationRequest | null>(readPendingCreationRequest);
  const activeRequestRef = useRef(observedRequest);
  const [creationObservation, setCreationObservation] = useState<ProductConversationCreationObservation | null>(null);
  const [observationVersion, setObservationVersion] = useState(0);
  const [creating, setCreating] = useState(observedRequest !== null);
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;
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

  const releaseCreation = useCallback((message: string | null, rotate = true) => {
    activeRequestRef.current = null;
    setObservedRequest(null);
    setCreationObservation(null);
    setCreating(false);
    setError(message);
    storePendingCreationRequest(null);
    if (rotate) {
      setRequestId(generateUUID());
      replayIntentRef.current = null;
      try {
        localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY);
        localStorage.removeItem(REPLAY_CREATE_INTENT_KEY);
      } catch { /* best effort after authoritative release */ }
    }
  }, []);

  const submitCreationRequest = useCallback(async (uncertain: Extract<PendingCreationRequest, { kind: 'uncertain' }>, surfaceExpansionError = false) => {
    const isCurrent = () => mountedRef.current && activeRequestRef.current === uncertain;
    try {
      const response = await api.createProductConversation(uncertain.request);
      if (!isCurrent()) return;
      if (response.request_id !== uncertain.request.request_id) throw new Error('Creation acceptance did not match the submitted request.');
      const accepted: PendingCreationRequest = {
        version: 2, kind: 'accepted', requestId: response.request_id, productConversationId: response.product_conversation_id,
      };
      activeRequestRef.current = accepted;
      storePendingCreationRequest(accepted);
      setObservedRequest(accepted);
    } catch (err) {
      if (!isCurrent()) return;
      if (err instanceof ExpansionError || (err instanceof ApiResponseError && err.status >= 400 && err.status < 500)) {
        const conflict = err instanceof ApiResponseError && err.status === 409;
        releaseCreation(conflict
          ? 'This request ID conflicts with an existing creation. Submit again to use a new request ID.'
          : err.message, conflict);
        if (surfaceExpansionError && err instanceof ExpansionError) throw err;
      } else {
        setError(`Creation acceptance is uncertain; checking this exact request. ${err instanceof Error ? err.message : ''}`);
        setObservedRequest(uncertain);
      }
    }
  }, [releaseCreation]);

  useEffect(() => {
    if (!observedRequest) return;
    const controller = new AbortController();
    let timeout: number | undefined;
    const observedRequestId = creationRequestId(observedRequest);
    const isCurrent = () => !controller.signal.aborted && activeRequestRef.current !== null
      && creationRequestId(activeRequestRef.current) === observedRequestId;
    const check = async () => {
      try {
        const result = await api.getProductConversationCreation(observedRequestId, controller.signal);
        if (!isCurrent()) return;
        const active = activeRequestRef.current;
        const expectedProductId = active?.kind === 'accepted' ? active.productConversationId : null;
        if (result.request_id !== observedRequestId
          || (expectedProductId !== null && result.product_conversation_id !== expectedProductId)) {
          throw new Error('Creation status did not match the submitted request. Continuing to observe the original request.');
        }
        if (expectedProductId === null) {
          const acceptedRequest: PendingCreationRequest = {
            version: 2, kind: 'accepted', requestId: result.request_id, productConversationId: result.product_conversation_id,
          };
          activeRequestRef.current = acceptedRequest;
          storePendingCreationRequest(acceptedRequest);
        }
        setCreationObservation(result);
        setError(null);
        if (result.kind === 'published') {
          setCreating(false);
          setDraft('');
          setImages([]);
          setFiles([]);
          setRecoveredLlmLanguage(null);
          clearNewConversationDraft();
          storePendingCreationRequest(null);
          try {
            localStorage.removeItem(RECOVERED_LLM_LANGUAGE_KEY);
            localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY);
            localStorage.removeItem(REPLAY_CREATE_INTENT_KEY);
          } catch { /* publication is already durable */ }
          navigateRef.current(result.canonical_route);
          return;
        }
        if (result.kind === 'failed' || result.kind === 'cancelled') {
          setCreating(false);
          setError(result.error || (result.kind === 'cancelled' ? 'Conversation creation cancelled.' : 'Conversation creation failed.'));
          return;
        }
      } catch (err) {
        if (!isCurrent()) return;
        if (err instanceof ApiResponseError && err.status === 404) {
          const active = activeRequestRef.current;
          if (active?.kind === 'accepted') {
            releaseCreation('This conversation creation was removed from the server. You can start a new conversation.');
            return;
          }
          if (active?.kind === 'uncertain') {
            // A late initial POST can commit after this GET: replay only the stored intent.
            await submitCreationRequest(active);
          }
        } else {
          setError(`Unable to observe this creation; checking again. ${err instanceof Error ? err.message : ''}`);
        }
      }
      if (isCurrent()) timeout = window.setTimeout(() => void check(), CREATION_OBSERVATION_INTERVAL_MS);
    };
    void check();
    return () => {
      controller.abort();
      if (timeout !== undefined) window.clearTimeout(timeout);
    };
  }, [observedRequest, observationVersion, releaseCreation, submitCreationRequest]);

  const resumeCreationObservation = useCallback((requestId: string) => {
    if (!activeRequestRef.current || creationRequestId(activeRequestRef.current) !== requestId) return;
    setCreationObservation(null);
    setCreating(true);
    setObservationVersion((version) => version + 1);
  }, []);

  const forgetCreationObservation = useCallback((requestId: string) => {
    if (!activeRequestRef.current || creationRequestId(activeRequestRef.current) !== requestId) return;
    if (!creationObservation || (creationObservation.kind !== 'failed' && creationObservation.kind !== 'cancelled')
      || creationObservation.allowed_actions.length !== 0) return;
    releaseCreation('Stopped observing this terminal creation locally. The server record has not been deleted. You can start a new conversation.');
  }, [creationObservation, releaseCreation]);

  const genericFilesEnabled = false;
  const hasMessageContent = draft.trim().length > 0 || images.length > 0 || files.length > 0;
  const canSend = hasMessageContent
    && observedRequest === null
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
    if (activeRequestRef.current || creating || dirStatus === 'invalid' || dirStatus === 'checking') return;

    setError(null);
    setCreating(true);

    try {
      const trimmedCwd = cwd.trim();
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
      const immutableIntent = JSON.stringify({
        cwd: trimmedCwd,
        objective: trimmed,
        model: selectedModel,
        effort: selectedEffort,
        llm_language: recoveredLlmLanguage,
        images,
      });
      let activeRequestId = requestId;
      if (replayIntentRef.current !== null && replayIntentRef.current !== immutableIntent) {
        activeRequestId = generateUUID();
        setRequestId(activeRequestId);
        try { localStorage.setItem(REPLAY_CREATE_REQUEST_KEY, activeRequestId); } catch { /* in-memory identity remains stable */ }
      }
      replayIntentRef.current = immutableIntent;
      try { localStorage.setItem(REPLAY_CREATE_INTENT_KEY, immutableIntent); } catch { /* in-memory intent remains stable */ }
      const createRequest: CreateProductConversationRequest = {
        request_id: activeRequestId,
        cwd: trimmedCwd,
        objective: trimmed,
        model: selectedModel,
        effort: selectedEffort,
        llm_language: recoveredLlmLanguage,
        ...(images.length > 0 ? { images: images.map((image) => ({ ...image })) } : {}),
      };
      const uncertain: Extract<PendingCreationRequest, { kind: 'uncertain' }> = { version: 2, kind: 'uncertain', request: createRequest };
      // Do not issue a POST unless reload recovery has its complete immutable intent.
      localStorage.setItem(PENDING_CREATE_REQUEST_KEY, JSON.stringify(uncertain));
      activeRequestRef.current = uncertain;
      await submitCreationRequest(uncertain, true);
    } catch (err) {
      if (!mountedRef.current) return;
      if (err instanceof ExpansionError) throw err;
      setCreating(false);
      setError(err instanceof Error ? err.message : 'Failed to persist creation intent; no request was sent.');
    }
  };

  const startOverFromCreation = useCallback((rowRequestId: string, creation: {
    cwd: string;
    objective: string;
    model: string | null;
    effort: ModelEffort | null;
    images: ImageData[];
    llm_language: string | null;
  }) => {
    if (activeRequestRef.current && creationRequestId(activeRequestRef.current) !== rowRequestId) return;
    activeRequestRef.current = null;
    setObservedRequest(null);
    setCreationObservation(null);
    setCreating(false);
    storePendingCreationRequest(null);
    const nextRequestId = generateUUID();
    setRequestId(nextRequestId);
    replayIntentRef.current = null;
    setCwd(creation.cwd);
    setDraft(creation.objective);
    setImages(creation.images ?? []);
    setFiles([]);
    setRecoveredLlmLanguage(creation.llm_language);
    selectModel(creation.model);
    selectEffort(creation.effort);
    try {
      if (creation.llm_language) {
        localStorage.setItem(RECOVERED_LLM_LANGUAGE_KEY, creation.llm_language);
      } else {
        localStorage.removeItem(RECOVERED_LLM_LANGUAGE_KEY);
      }
    } catch { /* replay still retains the pinned language in memory */ }
    setError(null);
    try {
      localStorage.removeItem(REPLAY_CREATE_REQUEST_KEY);
      localStorage.removeItem(REPLAY_CREATE_INTENT_KEY);
    } catch { /* best effort */ }
  }, [selectEffort, selectModel]);

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
    startOverFromCreation,
    creationObservation,
    activeCreationRequestId: activeRequestRef.current ? creationRequestId(activeRequestRef.current) : null,
    resumeCreationObservation,
    forgetCreationObservation,
  };
}
