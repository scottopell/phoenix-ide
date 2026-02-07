// serviceWorkerRegistration.ts
export async function register() {
  if ('serviceWorker' in navigator) {
    try {
      // Wait for window load to not block initial render
      window.addEventListener('load', async () => {
        const registration = await navigator.serviceWorker.register('/service-worker.js');
        console.log('[SW] Registration successful:', registration.scope);

        // Check for updates periodically
        setInterval(() => {
          registration.update();
        }, 60 * 60 * 1000); // Every hour

        // Handle updates
        registration.addEventListener('updatefound', () => {
          const newWorker = registration.installing;
          if (newWorker) {
            newWorker.addEventListener('statechange', () => {
              if (newWorker.state === 'installed' && navigator.serviceWorker.controller) {
                // New content available, show notification
                showUpdateNotification(newWorker);
              }
            });
          }
        });

        // Handle controller change (new SW activated)
        // Only reload if we already had a controller (meaning this is an update, not initial install)
        let hadController = !!navigator.serviceWorker.controller;
        navigator.serviceWorker.addEventListener('controllerchange', () => {
          if (hadController) {
            // Reload the page to get fresh content from the new SW
            window.location.reload();
          }
          // After first controller change, set flag so future changes trigger reload
          hadController = true;
        });
      });
    } catch (error) {
      console.error('[SW] Registration failed:', error);
    }
  }
}

export async function unregister() {
  if ('serviceWorker' in navigator) {
    try {
      const registrations = await navigator.serviceWorker.getRegistrations();
      for (const registration of registrations) {
        await registration.unregister();
      }
    } catch (error) {
      console.error('[SW] Unregistration failed:', error);
    }
  }
}

function showUpdateNotification(worker: ServiceWorker) {
  // Emit custom event that the app can listen to
  window.dispatchEvent(new CustomEvent('sw-update-available', {
    detail: { worker }
  }));
}

export async function clearServiceWorkerCache(): Promise<void> {
  if ('serviceWorker' in navigator && navigator.serviceWorker.controller) {
    return new Promise((resolve) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => resolve();
      navigator.serviceWorker.controller!.postMessage(
        { type: 'CLEAR_CACHE' },
        [channel.port2]
      );
    });
  }
}