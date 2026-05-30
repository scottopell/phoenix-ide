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

export type NewConversationWorkflow =
  | { kind: 'direct' }
  | { kind: 'planFromBranch'; baseBranch: string | null }
  | { kind: 'planFromTask'; task: TaskEntry | null; baseBranch: string | null }
  | { kind: 'continueBranch'; branch: string | null };

function workflowNeedsGit(workflow: NewConversationWorkflow): boolean {
  return workflow.kind !== 'direct';
}

function workflowTask(workflow: NewConversationWorkflow): TaskEntry | null {
  return workflow.kind === 'planFromTask' ? workflow.task : null;
}

function workflowBranch(workflow: NewConversationWorkflow): string | null {
  switch (workflow.kind) {
    case 'planFromBranch':
      return workflow.baseBranch;
    case 'planFromTask':
      return workflow.baseBranch;
    case 'continueBranch':
      return workflow.branch;
    case 'direct':
      return null;
  }
}

// The selected workflow is a pure function of (user override, git status,
// default branch) rather than a piece of state reconciled by effects. A null
// override means "follow the default", so a git repo can never render with
// 'direct' selected unless the user explicitly chose it. A null branch on an
// override means "still follow the default branch" and is filled in once
// branch metadata loads.
export function effectiveWorkflow(
  override: NewConversationWorkflow | null,
  isGitDir: boolean | null,
  fallbackBranch: string | null,
): NewConversationWorkflow {
  if (isGitDir !== true) return { kind: 'direct' };
  if (!override) return { kind: 'planFromBranch', baseBranch: fallbackBranch };
  switch (override.kind) {
    case 'planFromBranch':
      return { ...override, baseBranch: override.baseBranch ?? fallbackBranch };
    case 'planFromTask':
      return { ...override, baseBranch: override.baseBranch ?? fallbackBranch };
    case 'continueBranch':
      return { ...override, branch: override.branch ?? fallbackBranch };
    case 'direct':
      return override;
  }
}

function deriveSubmission(workflow: NewConversationWorkflow): { mode: 'direct' | 'managed' | 'branch'; baseBranch: string | null } {
  switch (workflow.kind) {
    case 'direct':
      return { mode: 'direct', baseBranch: null };
    case 'planFromBranch':
    case 'planFromTask':
      return { mode: 'managed', baseBranch: workflowBranch(workflow) };
    case 'continueBranch':
      return { mode: 'branch', baseBranch: workflow.branch };
  }
}

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
  // Only the user's deliberate choice is stored; the active workflow is derived
  // from this plus git status via effectiveWorkflow. null = follow the default.
  const [workflowOverride, setWorkflowOverride] = useState<NewConversationWorkflow | null>(null);
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [branches, setBranches] = useState<GitBranchEntry[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [gitMetadataLoading, setGitMetadataLoading] = useState(false);
  const [branchSearch, setBranchSearch] = useState('');
  const [branchSearchLoading, setBranchSearchLoading] = useState(false);

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');
  const metadataRequestSeqRef = useRef(0);

  const workflow = effectiveWorkflow(workflowOverride, isGitDir, defaultBranch ?? currentBranch);

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

  // A new directory drops any prior workflow choice; the active workflow then
  // re-derives from the new git status (and the metadata fetched below).
  useEffect(() => {
    setBranches([]);
    setTasks([]);
    setCurrentBranch(null);
    setDefaultBranch(null);
    setGitMetadataLoading(false);
    setWorkflowOverride(null);
    setBranchSearch('');
  }, [cwd]);

  useEffect(() => {
    if (!isGitDir) {
      setBranches([]);
      setTasks([]);
      setCurrentBranch(null);
      setDefaultBranch(null);
      setGitMetadataLoading(false);
      setBranchSearch('');
      return;
    }
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd) return;

    const requestSeq = ++metadataRequestSeqRef.current;
    let cancelled = false;
    setGitMetadataLoading(true);
    Promise.allSettled([
      api.listGitBranches(trimmedCwd),
      api.listProjectTasks(trimmedCwd),
    ]).then(([branchesResult, tasksResult]) => {
      if (cancelled || requestSeq !== metadataRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;

      if (branchesResult.status === 'fulfilled') {
        const resp = branchesResult.value;
        setBranches(resp.branches);
        setCurrentBranch(resp.current);
        setDefaultBranch(resp.default_branch ?? null);
      } else {
        console.warn('Failed to fetch git branches:', branchesResult.reason);
        setBranches([]);
        setCurrentBranch(null);
        setDefaultBranch(null);
      }

      if (tasksResult.status === 'fulfilled') {
        setTasks(tasksResult.value.tasks);
      } else {
        console.warn('Failed to fetch tasks:', tasksResult.reason);
        setTasks([]);
      }
    }).finally(() => {
      if (!cancelled && requestSeq === metadataRequestSeqRef.current) setGitMetadataLoading(false);
    });

    return () => { cancelled = true; };
  }, [isGitDir, cwd]);

  useEffect(() => {
    if (!isGitDir || !branchSearch.trim()) return;
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
  }, [isGitDir, cwd, branchSearch]);

  const selectedTask = workflowTask(workflow);
  const selectedBranchConflict = workflow.kind === 'continueBranch' && workflow.branch
    ? branches.find(b => b.name === workflow.branch)?.conflict_slug ?? null
    : null;
  const selectedTaskConflict = selectedTask?.conversation_slug ?? null;
  const selectedConflictSlug = selectedTaskConflict ?? selectedBranchConflict;
  const hasWorkflowStartingPoint = workflow.kind === 'direct' || Boolean(workflowBranch(workflow));
  const hasMessageContent = draft.trim().length > 0 || images.length > 0 || Boolean(selectedTask);
  const taskWorkflowReady = workflow.kind !== 'planFromTask' || Boolean(selectedTask);
  const gitWorkflowReady = !workflowNeedsGit(workflow)
    || (isGitDir === true && !gitMetadataLoading && !branchSearchLoading && hasWorkflowStartingPoint && taskWorkflowReady);

  const canSend = hasMessageContent
    && !creating
    && dirStatus !== 'invalid'
    && dirStatus !== 'checking'
    && gitWorkflowReady
    && !selectedConflictSlug;

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
    const taskStartProvidesContent = Boolean(selectedTask);
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
      if (workflowNeedsGit(workflow) && isGitDir !== true) {
        setError('Choose a Git repository before starting an isolated workflow.');
        setCreating(false);
        return;
      }
      if (workflowNeedsGit(workflow) && (gitMetadataLoading || branchSearchLoading)) {
        setError('Still loading Git branches. Try again in a moment.');
        setCreating(false);
        return;
      }
      const submission = deriveSubmission(workflow);
      if (workflowNeedsGit(workflow) && !submission.baseBranch) {
        setError('Pick a Git branch to start from.');
        setCreating(false);
        return;
      }
      if (workflow.kind === 'planFromTask' && !selectedTask) {
        setError('Pick a task file before starting from a task.');
        setCreating(false);
        return;
      }
      if (selectedConflictSlug) {
        setError('That starting point already has an active conversation.');
        setCreating(false);
        return;
      }
      const submitText = selectedTask
        ? buildTaskStartPrompt(trimmedCwd, selectedTask, trimmed)
        : trimmed;
      const conv = await createConversationWithStore(
        trimmedCwd, submitText, messageId, selectedModel || undefined, images, submission.mode,
        submission.baseBranch,
      );
      addRecentDir(trimmedCwd);
      setRecentDirs(getRecentDirs());
      navigate(`/c/${conv.slug}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
      setCreating(false);
    }
  };

  const setWorkflowFromUser = (next: NewConversationWorkflow) => {
    setWorkflowOverride(next);
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
    workflow,
    setWorkflow: setWorkflowFromUser,
    tasks,
    branches,
    currentBranch,
    defaultBranch,
    gitMetadataLoading,
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
