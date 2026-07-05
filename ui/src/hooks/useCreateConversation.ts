import { useState, useEffect, useRef, useCallback } from 'react';
import { api, ExpansionError, MAX_FILE_ATTACHMENT_SIZE, MAX_FILE_ATTACHMENTS, MAX_TOTAL_FILE_ATTACHMENT_SIZE } from '../api';
import { subscribeModels } from '../modelsPoller';
import type { GitBranchEntry, ImageData, ModelsResponse, TaskEntry } from '../api';
import type { DirStatus } from '../components/SettingsFields';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { isWebSpeechSupported } from '../components/VoiceInput/VoiceRecorder';
import { generateUUID } from '../utils/uuid';
import { useCreateConversationWithStore } from '../conversation';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const RECENT_DIRS_KEY = 'phoenix-recent-dirs';
const NEW_CONVERSATION_DRAFT_KEY = 'phoenix-new-conversation-draft';
const MAX_RECENT = 5;

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
//
// branchUnavailable is the settled "the fetch finished and found no usable
// branch" signal (unborn or branchless repo). Only then does the default fall
// back to 'direct' — otherwise such a repo would be stuck on planFromBranch with
// no branch and Send permanently disabled. While the fetch is still pending the
// flag is false, so a normal repo never flashes 'direct'.
export function effectiveWorkflow(
  override: NewConversationWorkflow | null,
  isGitDir: boolean | null,
  fallbackBranch: string | null,
  branchUnavailable: boolean,
): NewConversationWorkflow {
  if (isGitDir !== true) return { kind: 'direct' };
  if (!override) {
    if (!fallbackBranch && branchUnavailable) return { kind: 'direct' };
    return { kind: 'planFromBranch', baseBranch: fallbackBranch };
  }
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
  const [draft, setDraft] = useState(readNewConversationDraft);
  const [images, setImages] = useState<ImageData[]>([]);
  const [files, setFiles] = useState<File[]>([]);
  const [isDragOver, setIsDragOver] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const [recentDirs, setRecentDirs] = useState<string[]>(() => getRecentDirs());
  // Only the user's deliberate choice is stored; the active workflow is derived
  // from this plus git status via effectiveWorkflow. null = follow the default.
  const [workflowOverride, setWorkflowOverride] = useState<NewConversationWorkflow | null>(null);
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [taskAvailabilityLoading, setTaskAvailabilityLoading] = useState(false);
  const [taskAvailable, setTaskAvailable] = useState<boolean | null>(null);
  const [tasksLoading, setTasksLoading] = useState(false);
  const [tasksLoaded, setTasksLoaded] = useState(false);
  const [branches, setBranches] = useState<GitBranchEntry[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [gitMetadataLoading, setGitMetadataLoading] = useState(false);
  // Set once a branch fetch settles with no usable branch (unborn/branchless
  // repo, or the fetch failed). Distinct from "fetch not started yet" so the
  // default can degrade to 'direct' only after we actually know there is no
  // branch — never during the initial load window.
  const [branchUnavailable, setBranchUnavailable] = useState(false);
  const [branchSearch, setBranchSearch] = useState('');
  const [branchSearchLoading, setBranchSearchLoading] = useState(false);

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');
  const metadataRequestSeqRef = useRef(0);
  const taskAvailabilityRequestSeqRef = useRef(0);
  const taskListRequestSeqRef = useRef(0);

  const workflow = effectiveWorkflow(workflowOverride, isGitDir, defaultBranch ?? currentBranch, branchUnavailable);

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
  useEffect(() => { writeNewConversationDraft(draft); }, [draft]);

  // A new directory drops any prior workflow choice; the active workflow then
  // re-derives from the new git status (and the metadata fetched below).
  useEffect(() => {
    setBranches([]);
    setTasks([]);
    setTaskAvailabilityLoading(false);
    setTaskAvailable(null);
    setTasksLoading(false);
    setTasksLoaded(false);
    setCurrentBranch(null);
    setDefaultBranch(null);
    setGitMetadataLoading(false);
    setBranchUnavailable(false);
    setWorkflowOverride(null);
    setBranchSearch('');
  }, [cwd]);

  useEffect(() => {
    if (!isGitDir) {
      setBranches([]);
      setTasks([]);
      setTaskAvailabilityLoading(false);
      setTaskAvailable(null);
      setTasksLoading(false);
      setTasksLoaded(false);
      setCurrentBranch(null);
      setDefaultBranch(null);
      setGitMetadataLoading(false);
      setBranchUnavailable(false);
      setBranchSearch('');
      return;
    }
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd) return;

    const requestSeq = ++metadataRequestSeqRef.current;
    let cancelled = false;
    setGitMetadataLoading(true);
    setBranchUnavailable(false);
    api.listGitBranches(trimmedCwd).then(resp => {
      if (cancelled || requestSeq !== metadataRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      setBranches(resp.branches);
      setCurrentBranch(resp.current);
      setDefaultBranch(resp.default_branch ?? null);
      // Unborn/branchless repo: HEAD points at no commit, so neither a
      // default nor a current branch resolves. Record it so the default
      // workflow degrades to 'direct' instead of a branchless planFromBranch
      // the user can't send.
      setBranchUnavailable(!(resp.default_branch || resp.current));
    }).catch(err => {
      if (cancelled || requestSeq !== metadataRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      console.warn('Failed to fetch git branches:', err);
      setBranches([]);
      setCurrentBranch(null);
      setDefaultBranch(null);
      setBranchUnavailable(true);
    }).finally(() => {
      if (!cancelled && requestSeq === metadataRequestSeqRef.current) setGitMetadataLoading(false);
    });

    return () => { cancelled = true; };
  }, [isGitDir, cwd]);

  useEffect(() => {
    if (!isGitDir) return;
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd) return;

    const requestSeq = ++taskAvailabilityRequestSeqRef.current;
    let cancelled = false;
    setTaskAvailabilityLoading(true);
    setTaskAvailable(null);
    setTasks([]);
    setTasksLoaded(false);
    setTasksLoading(false);
    api.getProjectTaskAvailability(trimmedCwd).then(resp => {
      if (cancelled || requestSeq !== taskAvailabilityRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      setTaskAvailable(resp.available);
    }).catch(err => {
      if (cancelled || requestSeq !== taskAvailabilityRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      console.warn('Failed to check task availability:', err);
      setTaskAvailable(false);
    }).finally(() => {
      if (!cancelled && requestSeq === taskAvailabilityRequestSeqRef.current) setTaskAvailabilityLoading(false);
    });

    return () => { cancelled = true; };
  }, [isGitDir, cwd]);

  const loadProjectTasks = useCallback(() => {
    if (!isGitDir || tasksLoading || tasksLoaded) return;
    const trimmedCwd = cwd.trim();
    if (!trimmedCwd || taskAvailable === false) return;

    const requestSeq = ++taskListRequestSeqRef.current;
    setTasksLoading(true);
    api.listProjectTasks(trimmedCwd).then(resp => {
      if (requestSeq !== taskListRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      setTasks(resp.tasks);
      setTasksLoaded(true);
    }).catch(err => {
      if (requestSeq !== taskListRequestSeqRef.current || cwd.trim() !== trimmedCwd) return;
      console.warn('Failed to fetch tasks:', err);
      setTasks([]);
      setTasksLoaded(true);
    }).finally(() => {
      if (requestSeq === taskListRequestSeqRef.current) setTasksLoading(false);
    });
  }, [cwd, isGitDir, taskAvailable, tasksLoaded, tasksLoading]);

  useEffect(() => {
    if (workflow.kind === 'planFromTask') loadProjectTasks();
  }, [loadProjectTasks, workflow.kind]);

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
  const hasMessageContent = draft.trim().length > 0 || images.length > 0 || files.length > 0 || Boolean(selectedTask);
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

  const addFiles = async (dropped: File[]) => {
    const unsupportedImage = dropped.find(file => file.type.startsWith('image/') && !SUPPORTED_IMAGE_TYPES.includes(file.type));
    if (unsupportedImage) {
      setError(`${unsupportedImage.name} is not a supported image attachment type.`);
      return;
    }
    const genericFiles = dropped.filter(file => !SUPPORTED_IMAGE_TYPES.includes(file.type));
    const imageFiles = dropped.filter(file => SUPPORTED_IMAGE_TYPES.includes(file.type));
    if (imageFiles.length > 0) await addImages(imageFiles);
    if (genericFiles.length === 0) return;
    const tooLarge = genericFiles.find(file => file.size > MAX_FILE_ATTACHMENT_SIZE);
    if (tooLarge) {
      setError(`${tooLarge.name} exceeds the 10 MB file attachment limit.`);
      return;
    }
    if (files.length + genericFiles.length > MAX_FILE_ATTACHMENTS) {
      setError(`A message can include at most ${MAX_FILE_ATTACHMENTS} files.`);
      return;
    }
    const total = files.reduce((sum, file) => sum + file.size, 0)
      + genericFiles.reduce((sum, file) => sum + file.size, 0);
    if (total > MAX_TOTAL_FILE_ATTACHMENT_SIZE) {
      setError('Attachments exceed the 25 MB total limit.');
      return;
    }
    setFiles(prev => [...prev, ...genericFiles]);
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
    if (!trimmed && images.length === 0 && files.length === 0 && !taskStartProvidesContent) return;
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
      const conv = files.length > 0
        ? await createConversationWithStore(
            trimmedCwd,
            submitText,
            messageId,
            selectedModel || undefined,
            images,
            submission.mode,
            submission.baseBranch,
            undefined,
            undefined,
            files,
          )
        : await createConversationWithStore(
            trimmedCwd,
            submitText,
            messageId,
            selectedModel || undefined,
            images,
            submission.mode,
            submission.baseBranch,
          );
      addRecentDir(trimmedCwd);
      setRecentDirs(getRecentDirs());
      setDraft('');
      setImages([]);
      setFiles([]);
      clearNewConversationDraft();
      navigate(`/c/${conv.slug}`);
    } catch (err) {
      setCreating(false);
      // An unresolvable @reference in the first message rejects with a 422.
      // Re-throw so the composer can surface it inline next to the input
      // (REQ-IR-007) rather than as a page-level error.
      if (err instanceof ExpansionError) {
        throw err;
      }
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
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
    files,
    isDragOver,
    setIsDragOver,
    error,
    creating,
    canSend,
    workflow,
    setWorkflow: setWorkflowFromUser,
    tasks,
    taskAvailabilityLoading,
    taskAvailable,
    tasksLoading,
    tasksLoaded,
    loadProjectTasks,
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
    addFiles,
    removeFile,
    voiceSupported,
    handleVoiceFinal,
    handleVoiceInterim,
    textareaValue,
    updateDraft,
    selectedConflictSlug,
    handleSend,
    // The create-time mode + branch for the current workflow. Exposed so the
    // composer's inline-reference discovery resolves against the SAME root the
    // first message will expand against (one mapping, no drift).
    submission: deriveSubmission(workflow),
  };
}
