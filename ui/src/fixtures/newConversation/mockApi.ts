import { api } from '../../api';
import type { NewConversationScenario } from './types';

export function installNewConversationFixtureApi(scenario: NewConversationScenario): () => void {
  const original = {
    getEnv: api.getEnv,
    listArchivedConversations: api.listArchivedConversations,
    listConversations: api.listConversations,
    listDirectory: api.listDirectory,
    listModels: api.listModels,
    listProjectSkills: api.listProjectSkills,
    searchProjectFiles: api.searchProjectFiles,
    validateCwd: api.validateCwd,
  };

  api.getEnv = async () => ({ home_dir: '/Users/alex' });
  api.listArchivedConversations = async () => [];
  api.listConversations = async () => [];
  api.listDirectory = async () => ({ entries: [] });
  api.listModels = async () => scenario.models;
  api.listProjectSkills = async () => ({ skills: [] });
  api.searchProjectFiles = async () => ({ items: [] });
  api.validateCwd = async () => ({ valid: true, is_git: true });

  return () => {
    api.getEnv = original.getEnv;
    api.listArchivedConversations = original.listArchivedConversations;
    api.listConversations = original.listConversations;
    api.listDirectory = original.listDirectory;
    api.listModels = original.listModels;
    api.listProjectSkills = original.listProjectSkills;
    api.searchProjectFiles = original.searchProjectFiles;
    api.validateCwd = original.validateCwd;
  };
}
