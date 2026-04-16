import { useState, useEffect, useRef } from 'react';
import { api } from '../api';
import { getModels } from '../modelsPoller';
import type { GitBranchEntry, ImageData, ModelsResponse } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const RECENT_DIRS_KEY = 'phoenix-recent-dirs';
const MAX_RECENT = 5;

function getRecentDirs(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_DIRS_KEY) || '[]');
  } catch { return []; }
}

function addRecentDir(dir: string) {
  const recent = getRecentDirs().filter(d => d !== dir);
  recent.unshift(dir);
  localStorage.setItem(RECENT_DIRS_KEY, JSON.stringify(recent.slice(0, MAX_RECENT)));
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
  const [showAllModels, setShowAllModels] = useState(false);
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const [recentDirs, setRecentDirs] = useState<string[]>(() => getRecentDirs());
  const [mode, setMode] = useState<'direct' | 'managed' | 'branch'>('direct');
  const [branches, setBranches] = useState<GitBranchEntry[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [baseBranch, setBaseBranch] = useState<string | null>(null);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [branchSearch, setBranchSearch] = useState('');
  const [branchSearchLoading, setBranchSearchLoading] = useState(false);

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');

  // Load models (via shared cache — dedupes with other callers that may be
  // mounted concurrently) and environment info once on mount.
  useEffect(() => {
    getModels().then(modelsData => {
      setModels(modelsData);
      // Set default only if user has no saved preference
      setSelectedModel(prev => prev ?? modelsData.default);
    }).catch(console.error);
    api.getEnv().then(env => {
      setHomeDir(env.home_dir);
      if (!localStorage.getItem(LAST_CWD_KEY)) {
        setCwd(env.home_dir);
      }
    }).catch(console.error);
  }, []);

  // Save preferences
  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  // Reset to Direct when directory is not a git repo (Managed/Branch require git)
  useEffect(() => { if (isGitDir === false) setMode('direct'); }, [isGitDir]);

  // Fetch local branches when git dir is confirmed and mode needs branches (instant, no network)
  useEffect(() => {
    if (!isGitDir || (mode !== 'managed' && mode !== 'branch')) {
      setBranches([]);
      setCurrentBranch(null);
      setBaseBranch(null);
      setDefaultBranch(null);
      setBranchSearch('');
      return;
    }
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd) return;

    let cancelled = false;
    api.listGitBranches(trimmedCwd).then(resp => {
      if (cancelled) return;
      setBranches(resp.branches);
      setCurrentBranch(resp.current);
      setDefaultBranch(resp.default_branch ?? null);
      setBaseBranch(null);
    }).catch(err => {
      if (cancelled) return;
      console.warn('Failed to fetch git branches:', err);
      setBranches([]);
      setCurrentBranch(null);
      setDefaultBranch(null);
      setBaseBranch(null);
    });

    return () => { cancelled = true; };
  }, [isGitDir, mode, cwd]);

  // Debounced remote search when user types in the branch picker
  useEffect(() => {
    if (!isGitDir || (mode !== 'managed' && mode !== 'branch') || !branchSearch.trim()) return;
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd) return;

    setBranchSearchLoading(true);
    const timer = setTimeout(() => {
      let cancelled = false;
      api.listGitBranches(trimmedCwd, branchSearch.trim()).then(resp => {
        if (cancelled) return;
        setBranches(resp.branches);
        setBranchSearchLoading(false);
      }).catch(err => {
        if (cancelled) return;
        console.warn('Branch search failed:', err);
        setBranchSearchLoading(false);
      });
      // Stash the cancel fn for cleanup -- the timer already fired,
      // but the fetch might still be in flight.
      return () => { cancelled = true; };
    }, 300);

    return () => { clearTimeout(timer); setBranchSearchLoading(false); };
  }, [isGitDir, mode, cwd, branchSearch]);

  const canSend = (draft.trim().length > 0 || images.length > 0) && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking';

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

  /** Update draft and clear any active voice interim state */
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
    if (!trimmed && images.length === 0) return;
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
      const trimmedCwd = cwd.trim();
      const conv = await api.createConversation(
        trimmedCwd, trimmed, messageId, selectedModel || undefined, images, mode,
        (mode === 'managed' || mode === 'branch') ? baseBranch : null,
      );
      addRecentDir(trimmedCwd);
      setRecentDirs(getRecentDirs());
      navigate(`/c/${conv.slug}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
      setCreating(false);
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
    setSelectedModel,
    showAllModels,
    setShowAllModels,
    draft,
    setDraft,
    images,
    error,
    creating,
    canSend,
    mode,
    setMode,
    branches,
    currentBranch,
    baseBranch,
    setBaseBranch,
    defaultBranch,
    branchSearch,
    setBranchSearch,
    branchSearchLoading,
    recentDirs,
    addImages,
    removeImage,
    voiceSupported,
    handleVoiceFinal,
    handleVoiceInterim,
    textareaValue,
    updateDraft,
    handleSend,
  };
}
