// FTUX-01: Dir status "checking..." flash on page load
//
// The NewConversationPage initializes dirStatus as 'checking', which causes
// a flash of "..." loading indicator in the settings bar before async
// validation completes. When a user has a previously-saved cwd in localStorage,
// the settings bar shows "checking..." for 300+ms (the validation debounce)
// before settling to the real status.
//
// The initial render should NOT show a "checking" state -- it should either
// start with the default directory already validated (optimistic), or defer
// the status display until validation completes.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';

// Mock the API -- validateCwd is async, so it won't resolve during initial render
vi.mock('../api', () => ({
  api: {
    listModels: vi.fn().mockResolvedValue({ models: [{ id: 'claude-3-5-sonnet' }], default: 'claude-3-5-sonnet' }),
    getEnv: vi.fn().mockResolvedValue({ home_dir: '/home/user' }),
    validateCwd: vi.fn().mockResolvedValue({ valid: true }),
    listDirectory: vi.fn().mockResolvedValue({ entries: [] }),
  },
}));

describe('FTUX-01: Dir status flash on page load', () => {
  beforeEach(() => {
    // Simulate a returning user who has a saved cwd from a previous session.
    // This is the scenario that triggers the "checking..." flash.
    localStorage.setItem('phoenix-last-cwd', '/home/user/projects');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
  });

  it('should NOT show "checking" status indicator on initial render when cwd is saved', () => {
    const { container } = render(
      <MemoryRouter>
        <NewConversationPage />
      </MemoryRouter>
    );

    // When a valid-looking path (starts with /) is in localStorage, the
    // DirectoryPicker starts in 'checking' state and shows "..." icon
    // for 300+ms while async validation runs.
    //
    // The settings-row button contains:
    //   <span class="settings-status status-checking">...</span>
    //
    // This is the flash that confuses users. The initial render should NOT
    // show a loading/checking indicator. It should either:
    //   (a) optimistically show 'exists' for a previously-validated path, or
    //   (b) show no status indicator until validation completes.
    const checkingElements = container.querySelectorAll('.status-checking');
    expect(checkingElements.length).toBe(0);
  });
});
