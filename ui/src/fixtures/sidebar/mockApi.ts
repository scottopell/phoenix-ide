import { api } from '../../api';
import type { CodexLoginPreflight, ModelsResponse, ProductConversationSnapshotView } from '../../api';
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
    getProductConversationSnapshot: api.getProductConversationSnapshot,
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
  api.listProductConversations = async () => ({ product_conversations: data.productConversations?.[scenarioId] ?? [], coordinator: null });
  api.getProductConversationSnapshot = async (productConversationId): Promise<ProductConversationSnapshotView> => {
    const row = data.productConversations?.[scenarioId]?.find((candidate) => candidate.product_conversation_id === productConversationId);
    if (!row) throw new Error(`Unknown product conversation fixture: ${productConversationId}`);

    const latestTranscriptRowId = row.latest_transcript_row_id;
    return {
      segments: [{ segment_ordinal: 0, transcript_row_id: latestTranscriptRowId, slug: row.canonical_root.slug, title: row.canonical_root.title, messages: [], handoff: null }],
      before: null,
      has_older: false,
      product_conversation_id: row.product_conversation_id,
      close: null,
      canonical_route: row.canonical_route,
      requested_transcript_row_id: latestTranscriptRowId,
      canonical_root: row.canonical_root,
      ordinary_lifecycle: 'open',
      latest_transcript_row_id: latestTranscriptRowId,
      writable_transcript_row_id: latestTranscriptRowId,
      latest_opening: { status: 'none' },
      updated_at: row.updated_at,
      presentation: { kind: 'state', display_name: row.presentation.display_name, presentation_mode: row.presentation.presentation_mode },
      work_identity: null,
      source: null,
      chain_qa_compatibility: null,
    };
  };
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
    api.getProductConversationSnapshot = original.getProductConversationSnapshot;
    api.archiveConversation = original.archiveConversation;
    api.archiveChain = original.archiveChain;
    api.renameProductConversation = original.renameProductConversation;
  };
}
