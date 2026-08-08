const RECOVERY_KEY = 'phoenix:module-recovery';
const RECOVERY_WINDOW_MS = 30_000;

type PreloadErrorEvent = Event & { payload?: unknown };

type RecoveryOptions = {
  now?: () => number;
  reload?: () => void;
  storage?: Pick<Storage, 'getItem' | 'setItem'>;
};

type RecoveryRecord = {
  attemptedAt: number;
};

function errorDetail(payload: unknown): string {
  if (payload instanceof Error) return payload.stack || payload.message;
  return String(payload ?? 'Unknown module preload error');
}

function recentAttempt(storage: Pick<Storage, 'getItem'>, now: number): boolean {
  try {
    const raw = storage.getItem(RECOVERY_KEY);
    if (!raw) return false;
    const record = JSON.parse(raw) as Partial<RecoveryRecord>;
    return typeof record.attemptedAt === 'number'
      && now - record.attemptedAt < RECOVERY_WINDOW_MS;
  } catch {
    return false;
  }
}

export function installDeploymentModuleRecovery(options: RecoveryOptions = {}): () => void {
  const now = options.now ?? Date.now;
  const reload = options.reload ?? (() => window.location.reload());
  const storage = options.storage ?? window.sessionStorage;

  const handlePreloadError = (event: Event) => {
    const preloadEvent = event as PreloadErrorEvent;
    const attemptedAt = now();

    console.error(
      '[Phoenix] Failed to load a deployed UI module.',
      errorDetail(preloadEvent.payload),
    );

    if (recentAttempt(storage, attemptedAt)) return;

    try {
      storage.setItem(RECOVERY_KEY, JSON.stringify({ attemptedAt } satisfies RecoveryRecord));
    } catch {
      return;
    }

    event.preventDefault();
    reload();
  };

  window.addEventListener('vite:preloadError', handlePreloadError);
  return () => window.removeEventListener('vite:preloadError', handlePreloadError);
}
