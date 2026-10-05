import { describe, it, expect, beforeEach } from 'vitest';
import {
  beginNewProductConversationIntent,
  reconcileSubscribedModelSelection,
} from './useCreateConversation';

describe('reconcileSubscribedModelSelection', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('retains effort when the resolved replacement model supports it', () => {
    const next = reconcileSubscribedModelSelection(
      {
        default: 'gpt-5.4',
        llm_configured: true,
        credential_status: 'valid',
        models: [{
          id: 'gpt-5.4',
          provider: 'openai',
          recommended: true,
          description: '',
          context_window: 200_000,
          effort_capabilities: { support: 'supported', levels: ['low'], native_default: { known: 'low' } },
        }],
      },
      'retired-model',
      'low',
    );

    expect(next).toEqual({ selectedModel: 'gpt-5.4', selectedEffort: 'low' });
  });

  it('clears effort when the resolved replacement model does not support it', () => {
    const next = reconcileSubscribedModelSelection(
      {
        default: 'gpt-5',
        llm_configured: true,
        credential_status: 'valid',
        models: [{
          id: 'gpt-5',
          provider: 'openai',
          recommended: true,
          description: '',
          context_window: 200_000,
          effort_capabilities: { support: 'unsupported' },
        }],
      },
      'retired-model',
      'low',
    );

    expect(next).toEqual({ selectedModel: 'gpt-5', selectedEffort: null });
  });

  it('uses the server default only when no valid remembered selection exists', () => {
    const data = {
      default: 'gpt-6.1-sol',
      llm_configured: true,
      credential_status: 'valid' as const,
      models: ['gpt-6.1-sol', 'gpt-6-astra'].map(id => ({
        id, provider: 'openai', recommended: true, description: '', context_window: 272_000,
        effort_capabilities: {
          support: 'supported' as const,
          levels: ['medium' as const, 'high' as const],
          native_default: { known: 'medium' as const },
        },
      })),
    };
    expect(reconcileSubscribedModelSelection(data, null, null))
      .toEqual({ selectedModel: 'gpt-6.1-sol', selectedEffort: null });
    expect(reconcileSubscribedModelSelection(data, 'gpt-6-astra', 'high'))
      .toEqual({ selectedModel: 'gpt-6-astra', selectedEffort: 'high' });
  });

  it('clears any pending product create request id when a new intent begins', () => {
    localStorage.setItem(
      'phoenix-pending-product-create-request',
      JSON.stringify({
        scope: JSON.stringify(['/repo', 'Retry safely', 'claude-3-5-sonnet', null]),
        requestId: 'req-stale',
      }),
    );

    localStorage.setItem('phoenix-create-llm-language', 'Japanese');

    beginNewProductConversationIntent();

    expect(localStorage.getItem('phoenix-pending-product-create-request')).toBeNull();
    expect(localStorage.getItem('phoenix-create-llm-language')).toBeNull();
  });
});
