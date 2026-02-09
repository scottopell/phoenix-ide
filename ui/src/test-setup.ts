// Test setup
import '@testing-library/jest-dom';

// Mock IndexedDB for tests
global.indexedDB = {} as unknown as IDBFactory;

// Mock navigator.storage
Object.defineProperty(navigator, 'storage', {
  writable: true,
  value: {
    estimate: () => Promise.resolve({ usage: 0, quota: 0 }),
  },
});
