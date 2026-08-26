import { describe, it, expect } from 'vitest';
import {
  reconcileSubscribedModelSelection,
} from './useCreateConversation';

describe('reconcileSubscribedModelSelection', () => {
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
});
