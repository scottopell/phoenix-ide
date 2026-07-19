import { describe, expect, it } from 'vitest';
import { findConversationFragmentElement } from './conversationFragmentElement';

describe('findConversationFragmentElement', () => {
  it('scopes duplicate fragment ids to the reveal target tool owner', () => {
    const row = document.createElement('div');
    row.innerHTML = `
      <div data-tool-id="read-a"><span data-fragment-id="read-file-path"></span></div>
      <div data-tool-id="read-b"><span data-fragment-id="read-file-path"></span></div>
    `;

    const selected = findConversationFragmentElement(row, 'read-file-path', {
      kind: 'tool-result-read-file',
      toolUseId: 'read-b',
      fragmentId: 'read-file-path',
    });

    expect(selected).toBe(row.querySelector('[data-tool-id="read-b"] [data-fragment-id="read-file-path"]'));
  });

  it('uses the row directly for fragments without a tool owner', () => {
    const row = document.createElement('div');
    row.innerHTML = '<span data-fragment-id="agent-text-0"></span>';

    expect(findConversationFragmentElement(row, 'agent-text-0', {
      kind: 'agent-text',
      key: 'agent-text-0',
    })).toBe(row.firstElementChild);
  });
});
