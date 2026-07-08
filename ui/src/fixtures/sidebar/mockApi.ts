import { api } from '../../api';
import type { CodexLoginPreflight, ModelsResponse } from '../../api';
import type { DeploymentInfo } from '../../generated/DeploymentInfo';
import type { SidebarFixtureData } from './types';

export function installSidebarFixtureApi(data: SidebarFixtureData) {
  const original = {
    codexLoginPreflight: api.codexLoginPreflight,
    deploymentInfo: api.deploymentInfo,
    getLocalServices: api.getLocalServices,
    getProjects: api.getProjects,
    listModels: api.listModels,
  };

  api.codexLoginPreflight = async (): Promise<CodexLoginPreflight> => ({
    auth_path: '/tmp/sidebar-fixture/auth.json',
    piggyback_path: '/tmp/sidebar-fixture/piggyback.json',
    already_signed_in: false,
    bridge_loaded_at_startup: false,
    restart_required_after_login: false,
    piggyback_env_set: false,
    account_id: null,
    account_email: null,
  });
  api.deploymentInfo = async (): Promise<DeploymentInfo> => ({ local_access: true } as unknown as DeploymentInfo);
  api.getLocalServices = async () => ({ services: [] });
  api.getProjects = async () => data.projects;
  api.listModels = async (): Promise<ModelsResponse> => ({
    models: [],
    default: '',
    llm_configured: false,
    credential_status: 'not_configured',
  });

  return () => {
    api.codexLoginPreflight = original.codexLoginPreflight;
    api.deploymentInfo = original.deploymentInfo;
    api.getLocalServices = original.getLocalServices;
    api.getProjects = original.getProjects;
    api.listModels = original.listModels;
  };
}
