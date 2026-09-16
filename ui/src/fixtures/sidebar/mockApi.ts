import { api } from '../../api';
import type { CodexLoginPreflight, ModelsResponse } from '../../api';
import type { DeploymentInfo } from '../../generated/DeploymentInfo';
import type { SidebarFixtureData } from './types';

export function installSidebarFixtureApi(data: SidebarFixtureData, scenarioId: keyof NonNullable<SidebarFixtureData['productConversations']>) {
  const original = {
    codexLoginPreflight: api.codexLoginPreflight,
    codexQuota: api.codexQuota,
    deploymentInfo: api.deploymentInfo,
    getLocalServices: api.getLocalServices,
    getProjects: api.getProjects,
    listModels: api.listModels,
    listProductConversations: api.listProductConversations,
    renameConversation: api.renameConversation,
    archiveConversation: api.archiveConversation,
    archiveChain: api.archiveChain,
    renameProductConversation: api.renameProductConversation,
  };

  api.codexLoginPreflight = async (): Promise<CodexLoginPreflight> => ({
    auth_path: '/tmp/sidebar-fixture/auth.json',
    already_signed_in: false,
    bridge_loaded_at_startup: false,
    restart_required_after_login: false,
    account_id: null,
    account_email: null,
  });
  api.codexQuota = async () => null;
  api.deploymentInfo = async (): Promise<DeploymentInfo> => ({ local_access: true } as unknown as DeploymentInfo);
  api.getLocalServices = async () => ({ services: [] });
  api.getProjects = async () => data.projects;
  api.listModels = async (): Promise<ModelsResponse> => ({
    models: [],
    default: '',
    llm_configured: false,
    credential_status: 'not_configured',
  });
  api.listProductConversations = async () => ({ product_conversations: data.productConversations?.[scenarioId] ?? [] });
  api.renameConversation = async (id: string, name: string) => ({
    conversation: {
      id,
      slug: name,
      title: name,
      model: 'fixture',
      cwd: '/tmp/sidebar-fixture',
      created_at: new Date(0).toISOString(),
      updated_at: new Date(0).toISOString(),
      message_count: 0,
      archived: false,
      browser_session_active: false,
      terminal_uses_tmux: false,
      work_scope_key: `conversation:${id}`,
    },
  });
  api.renameProductConversation = async (reference: string, title: string) => {
    const row = data.productConversations?.[scenarioId]?.find((candidate) => candidate.product_conversation_id === reference);
    if (!row) throw new Error(`Unknown product conversation fixture: ${reference}`);
    return {
      ...row,
      canonical_root: { ...row.canonical_root, title },
      presentation: { ...row.presentation, display_name: title },
    };
  };
  api.archiveConversation = async () => ({ ok: true });
  api.archiveChain = async () => undefined;

  return () => {
    api.codexLoginPreflight = original.codexLoginPreflight;
    api.codexQuota = original.codexQuota;
    api.deploymentInfo = original.deploymentInfo;
    api.getLocalServices = original.getLocalServices;
    api.getProjects = original.getProjects;
    api.listModels = original.listModels;
    api.listProductConversations = original.listProductConversations;
    api.renameConversation = original.renameConversation;
    api.archiveConversation = original.archiveConversation;
    api.archiveChain = original.archiveChain;
    api.renameProductConversation = original.renameProductConversation;
  };
}
