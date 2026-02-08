// serviceWorkerRegistration.ts
// Simplified: Only unregisters existing service workers to clean up from previous versions

export async function register() {
  // We no longer use service workers - unregister any existing ones
  await unregister();
}

export async function unregister() {
  if ('serviceWorker' in navigator) {
    try {
      const registrations = await navigator.serviceWorker.getRegistrations();
      for (const registration of registrations) {
        await registration.unregister();
        console.log('[SW] Unregistered service worker:', registration.scope);
      }
      // Also clear any SW caches
      if ('caches' in window) {
        const cacheNames = await caches.keys();
        for (const cacheName of cacheNames) {
          if (cacheName.startsWith('phoenix-')) {
            await caches.delete(cacheName);
            console.log('[SW] Deleted cache:', cacheName);
          }
        }
      }
    } catch (error) {
      console.error('[SW] Unregistration failed:', error);
    }
  }
}
