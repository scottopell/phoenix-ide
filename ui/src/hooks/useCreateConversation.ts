import { useState, useEffect, useRef } from 'react';
import { api } from '../api';
import { subscribeModels } from '../modelsPoller';
import type { GitBranchEntry, ImageData, ModelsResponse, TaskEntry } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';
import { useCreateConversationWithStore } from '../conversation';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const RECENT_DIRS_KEY = 'phoenix-recent-dirs';
const MAX_RECENT = 5;

function relativeTaskPath(cwd: string, taskPath: string): string {
  const root = cwd.endsWith('/') ? cwd : `${cwd}/`;
  return taskPath.startsWith(root) ? taskPath.slice(root.length) : taskPath;
}

function buildTaskStartPrompt(cwd: string, task: TaskEntry, extraInstructions: string): string {
  const taskFile = relativeTaskPath(cwd, task.path);
  const extra = extraInstructions.trim();
  return [
    `Start from the existing task file \`${taskFile}\`.`,
    '',
    `Call the propose_task tool with {"task_file":"${taskFile}"} as your only tool call so I can review and approve the task before Work mode begins.`,
    extra ? `\nAdditional context from me:\n${extra}` : '',
  ].filter(Boolean).join('\n');
}

export type ConversationIntent = 'direct' | 'fromExistingWork';
export type StartingPoint =
  | { kind: 'branch'; name: string }
  | { kind: 'checkoutBranch'; name: string }
  | { kind: 'task'; task: TaskEntry };

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
  const createConversationWithStore = useCreateConversationWithStore();
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
  const [intent, setIntent] = useState<ConversationIntent>('direct');
  const [startingPoint, setStartingPoint] = useState<StartingPoint | null>(null);
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [branches, setBranches] = useState<GitBranchEntry[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [baseBranch, setBaseBranch] = useState<string | null>(null);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [branchSearch, setBranchSearch] = useState('');
  const [branchSearchLoading, setBranchSearchLoading] = useState(false);

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');

  // Subscribe to the shared models poller so credential transitions
  // (Codex sign-in/sign-out, gateway flips) reach this page without a
  // manual refresh.
  useEffect(() => {
    const unsub = subscribeModels(modelsData => {
      setModels(modelsData);
      // Honor saved preference only if it's still a registered model. Without
      // this, a stale localStorage entry (e.g. a model that was the only
      // option at a previous deploy and has since been superseded) silently
      // sticks and the user submits against an unintended model. After a
      // sign-out the registered set may drop to empty — null out the
      // selection so the UI doesn't pin a now-invalid id.
      setSelectedModel(prev => {
        if (modelsData.models.length === 0) return null;
        return prev && modelsData.models.some(m => m.id === prev)
          ? prev
          : modelsData.default;
      });
    });
    api.getEnv().then(env => {
      setHomeDir(env.home_dir);
      if (!localStorage.getItem(LAST_CWD_KEY)) {
        setCwd(env.home_dir);
      }
    }).catch(console.error);
    return () => { unsub(); };
  }, []);

  // Save preferences
  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  useEffect(() => { if (isGitDir === false) setIntent('direct'); }, [isGitDir]);

  useEffect(() => {
    setBranches([]);
    setTasks([]);
    setCurrentBranch(null);
    setBaseBranch(null);
    setDefaultBranch(null);
    setStartingPoint(null);
    setBranchSearch('');
  }, [cwd]);

  useEffect(() => {
    if (!isGitDir || intent !== 'fromExistingWork') {
      setBranches([]);
      setTasks([]);
      setCurrentBranch(null);
      setBaseBranch(null);
      setDefaultBranch(null);
      setStartingPoint(null);
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
      const initialBranch = resp.default_branch ?? resp.current;
      setBaseBranch(initialBranch);
      setStartingPoint(prev => prev ?? (initialBranch ? { kind: 'branch', name: initialBranch } : null));
    }).catch(err => {
      if (cancelled) return;
      console.warn('Failed to fetch git branches:', err);
      setBranches([]);
      setCurrentBranch(null);
      setDefaultBranch(null);
      setBaseBranch(null);
    });
    api.listProjectTasks(trimmedCwd).then(resp => {
      if (cancelled) return;
      setTasks(resp.tasks);
    }).catch(err => {
      if (cancelled) return;
      console.warn('Failed to fetch tasks:', err);
      setTasks([]);
    });

    return () => { cancelled = true; };
  }, [isGitDir, intent, cwd]);

  useEffect(() => {
    if (!isGitDir || intent !== 'fromExistingWork' || !branchSearch.trim()) return;
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
      return () => { cancelled = true; };
    }, 300);

    return () => { clearTimeout(timer); setBranchSearchLoading(false); };
  }, [isGitDir, intent, cwd, branchSearch]);

  const activeStartingPoint = intent === 'fromExistingWork' ? startingPoint : null;
  const effectiveBranch = activeStartingPoint
    ? (activeStartingPoint.kind === 'branch' || activeStartingPoint.kind === 'checkoutBranch' ? activeStartingPoint.name : (baseBranch ?? currentBranch ?? defaultBranch))
    : null;
  const selectedBranchConflict = activeStartingPoint?.kind === 'checkoutBranch' && effectiveBranch
    ? branches.find(b => b.name === effectiveBranch)?.conflict_slug ?? null
    : null;
  const selectedTaskConflict = activeStartingPoint?.kind === 'task'
    ? activeStartingPoint.task.conversation_slug ?? null
    : null;
  const selectedConflictSlug = selectedTaskConflict ?? selectedBranchConflict;
  const hasStartingPoint = intent === 'direct' || Boolean(activeStartingPoint);
  const hasMessageContent = draft.trim().length > 0 || images.length > 0 || activeStartingPoint?.kind === 'task';

  const canSend = hasMessageContent && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking' && hasStartingPoint && !selectedConflictSlug;

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
    const taskStartProvidesContent = activeStartingPoint?.kind === 'task';
    if (!trimmed && images.length === 0 && !taskStartProvidesContent) return;
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
      if (intent === 'fromExistingWork' && !activeStartingPoint) {
        setError('Pick a branch or task to start from.');
        setCreating(false);
        return;
      }
      if (intent === 'fromExistingWork' && selectedConflictSlug) {
        setError('That starting point already has an active conversation.');
        setCreating(false);
        return;
      }
      const backendMode = intent === 'direct'
        ? 'direct'
        : activeStartingPoint?.kind === 'checkoutBranch'
          ? 'branch'
          : 'managed';
      const submitBranch = intent === 'fromExistingWork'
        ? (activeStartingPoint?.kind === 'branch' || activeStartingPoint?.kind === 'checkoutBranch' ? activeStartingPoint.name : (baseBranch ?? currentBranch ?? defaultBranch))
        : null;
      if (intent === 'fromExistingWork' && !submitBranch) {
        setError('Pick a Git starting point.');
        setCreating(false);
        return;
      }
      const submitText = activeStartingPoint?.kind === 'task'
        ? buildTaskStartPrompt(trimmedCwd, activeStartingPoint.task, trimmed)
        : trimmed;
      const conv = await createConversationWithStore(
        trimmedCwd, submitText, messageId, selectedModel || undefined, images, backendMode,
        submitBranch,
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
    intent,
    setIntent,
    startingPoint,
    setStartingPoint,
    tasks,
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
    selectedConflictSlug,
    handleSend,
  };
}
