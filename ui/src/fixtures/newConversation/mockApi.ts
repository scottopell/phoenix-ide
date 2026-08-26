import { api } from '../../api';
import type { NewConversationScenario } from './types';

export function installNewConversationFixtureApi(scenario: NewConversationScenario): () => void {
  const original = {
    getEnv: api.getEnv,
    reserveProductRoot: api.reserveProductRoot,
    listRecentManagementRootSuggestions: api.listRecentManagementRootSuggestions,
    listArchivedConversations: api.listArchivedConversations,
    listConversations: api.listConversations,
    listDirectory: api.listDirectory,
    listModels: api.listModels,
    listProjectSkills: api.listProjectSkills,
    searchProjectFiles: api.searchProjectFiles,
    validateCwd: api.validateCwd,
  };

  api.getEnv = async () => ({ home_dir: '/Users/alex' });
  api.reserveProductRoot = async (cwd) => ({
    kind: 'exact_committed_tree' as const,
    exact_checkout_oid: '1111111111111111111111111111111111111111',
    logical_base: 'main',
    freshness: 'fresh' as const,
    root_reservation: {
      repository_id: null, id: `fixture-reservation:${cwd}`,
      cwd,
      kind: 'exact_committed_tree',
      repo_root: cwd,
      exact_checkout_oid: 'fixture-oid',
      logical_base: 'main',
      freshness: 'fresh', unresolved_reason: null,
    },
  });
  api.listRecentManagementRootSuggestions = async () => ({ suggestions: [] });
  api.listArchivedConversations = async () => [];
  api.listConversations = async () => [];
  api.listDirectory = async () => ({ entries: [] });
  api.listModels = async () => scenario.models;
  api.listProjectSkills = async () => ({ skills: [] });
  api.searchProjectFiles = async () => ({ items: [] });
  api.validateCwd = async () => ({ valid: true, is_git: true });

  return () => {
    api.getEnv = original.getEnv;
    api.reserveProductRoot = original.reserveProductRoot;
    api.listRecentManagementRootSuggestions = original.listRecentManagementRootSuggestions;
    api.listArchivedConversations = original.listArchivedConversations;
    api.listConversations = original.listConversations;
    api.listDirectory = original.listDirectory;
    api.listModels = original.listModels;
    api.listProjectSkills = original.listProjectSkills;
    api.searchProjectFiles = original.searchProjectFiles;
    api.validateCwd = original.validateCwd;
  };
}
