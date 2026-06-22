const KEY_PREFIX = 'terminal-height:';
const COLLAPSED_SUFFIX = '-collapsed';

export function terminalPaneStorageKey(slug: string): string {
  return KEY_PREFIX + slug;
}

export function clearTerminalPaneStorage(slug: string): void {
  try {
    const key = terminalPaneStorageKey(slug);
    localStorage.removeItem(key);
    localStorage.removeItem(key + COLLAPSED_SUFFIX);
  } catch {
    // Safari private mode / quota: silent failure matches other UI storage cleanup.
  }
}
