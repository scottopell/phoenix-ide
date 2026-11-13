// Test setup
import '@testing-library/jest-dom';
import { beforeEach } from 'vitest';

// Mock IndexedDB for tests
global.indexedDB = {} as unknown as IDBFactory;

// Mock navigator.storage
Object.defineProperty(navigator, 'storage', {
  writable: true,
  value: {
    estimate: () => Promise.resolve({ usage: 0, quota: 0 }),
  },
});

// The product conversation list is a module-level store; isolate it per test
// without importing it here (that would bypass test files' `vi.mock('../api')`).
beforeEach(() => {
  (globalThis as { __phoenixResetProductConversationList?: () => void })
    .__phoenixResetProductConversationList?.();
});
