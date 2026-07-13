import { api } from '../../api';
import type { NewConversationScenario } from './types';

export function installNewConversationFixtureApi(scenario: NewConversationScenario): () => void {
  const original = {
    getEnv: api.getEnv,
    getProjectTaskAvailability: api.getProjectTaskAvailability,
    getProjects: api.getProjects,
    listArchivedConversations: api.listArchivedConversations,
    listConversations: api.listConversations,
    listDirectory: api.listDirectory,
    listGitBranches: api.listGitBranches,
    listModels: api.listModels,
    listProjectTasks: api.listProjectTasks,
    listProjectSkills: api.listProjectSkills,
    searchProjectFiles: api.searchProjectFiles,
    validateCwd: api.validateCwd,
  };

  api.getEnv = async () => ({ home_dir: '/Users/alex' });
  api.getProjectTaskAvailability = async () => ({ available: true });
  api.getProjects = async () => scenario.projects;
  api.listArchivedConversations = async () => [];
  api.listConversations = async () => [];
  api.listDirectory = async () => ({ entries: [] });
  api.listGitBranches = async () => ({
    branches: scenario.branches,
    current: scenario.currentBranch,
    default_branch: scenario.defaultBranch,
  });
  api.listModels = async () => scenario.models;
  api.listProjectTasks = async () => ({ tasks: scenario.tasks });
  api.listProjectSkills = async () => ({ skills: [] });
  api.searchProjectFiles = async () => ({ items: [] });
  api.validateCwd = async () => ({ valid: true, is_git: true });

  return () => {
    api.getEnv = original.getEnv;
    api.getProjectTaskAvailability = original.getProjectTaskAvailability;
    api.getProjects = original.getProjects;
    api.listArchivedConversations = original.listArchivedConversations;
    api.listConversations = original.listConversations;
    api.listDirectory = original.listDirectory;
    api.listGitBranches = original.listGitBranches;
    api.listModels = original.listModels;
    api.listProjectTasks = original.listProjectTasks;
    api.listProjectSkills = original.listProjectSkills;
    api.searchProjectFiles = original.searchProjectFiles;
    api.validateCwd = original.validateCwd;
  };
}
