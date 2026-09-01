import { api } from '../../api';
import type { NewConversationScenario } from './types';

export function installNewConversationFixtureApi(scenario: NewConversationScenario): () => void {
  const original = {
    getEnv: api.getEnv,
    listRecentManagementRootSuggestions: api.listRecentManagementRootSuggestions,
    listArchivedConversations: api.listArchivedConversations,
    listConversations: api.listConversations,
    listDirectory: api.listDirectory,
    listModels: api.listModels,
    listProductConversationCreations: api.listProductConversationCreations,
    retryProductConversationDelivery: api.retryProductConversationDelivery,
    listProjectSkills: api.listProjectSkills,
    searchProjectFiles: api.searchProjectFiles,
    validateCwd: api.validateCwd,
  };

  api.getEnv = async () => ({ home_dir: '/Users/alex' });
  api.listRecentManagementRootSuggestions = async () => ({ suggestions: [] });
  api.listArchivedConversations = async () => [];
  api.listConversations = async () => [];
  api.listDirectory = async () => ({ entries: [] });
  api.listModels = async () => scenario.models;
  api.listProductConversationCreations = async (cursor?: string) => ({
    product_creations: cursor ? [] : (scenario.recoveryRows ?? []),
    next_cursor: cursor ? null : (scenario.recoveryNextCursor ?? null),
  });
  api.retryProductConversationDelivery = async () => {};
  api.listProjectSkills = async () => ({ skills: [] });
  api.searchProjectFiles = async () => ({ items: [] });
  api.validateCwd = async () => ({ valid: true, is_git: true });

  return () => {
    api.getEnv = original.getEnv;
    api.listRecentManagementRootSuggestions = original.listRecentManagementRootSuggestions;
    api.listArchivedConversations = original.listArchivedConversations;
    api.listConversations = original.listConversations;
    api.listDirectory = original.listDirectory;
    api.listModels = original.listModels;
    api.listProductConversationCreations = original.listProductConversationCreations;
    api.retryProductConversationDelivery = original.retryProductConversationDelivery;
    api.listProjectSkills = original.listProjectSkills;
    api.searchProjectFiles = original.searchProjectFiles;
    api.validateCwd = original.validateCwd;
  };
}
