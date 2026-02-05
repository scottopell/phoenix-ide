// service-worker.js
const CACHE_NAME = 'phoenix-ide-v2';
const API_CACHE_NAME = 'phoenix-api-v1';
const CACHE_DURATION = 5 * 60 * 1000; // 5 minutes

// Assets to cache on install
const STATIC_ASSETS = [
  '/',
  '/index.html',
  // Note: Vite generates hashed filenames, so we'll cache them dynamically
];

// API endpoints to cache
const CACHEABLE_API_PATTERNS = [
  /\/api\/conversations$/,
  /\/api\/conversations\/[^/]+$/,
  /\/api\/conversations\/by-slug\//,
  /\/api\/models$/,
  /\/api\/cwd\/validate$/,
  /\/api\/cwd\/list$/,
];

// API endpoints to never cache
const SKIP_CACHE_PATTERNS = [
  /\/api\/conversations\/[^/]+\/events$/, // SSE endpoints
  /\/api\/conversations\/[^/]+\/messages$/, // POST endpoints
  /\/api\/conversations\/[^/]+\/cancel$/, // Action endpoints
];

// Install event - cache static assets
self.addEventListener('install', (event) => {
  console.log('[ServiceWorker] Install');
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => {
      console.log('[ServiceWorker] Caching static assets');
      // Try to cache static assets, but don't fail install if some are missing
      return Promise.allSettled(
        STATIC_ASSETS.map(url => 
          cache.add(url).catch(err => console.log(`Failed to cache ${url}:`, err))
        )
      );
    })
  );
  // Skip waiting to activate immediately
  self.skipWaiting();
});

// Activate event - clean up old caches
self.addEventListener('activate', (event) => {
  console.log('[ServiceWorker] Activate');
  event.waitUntil(
    caches.keys().then((cacheNames) => {
      return Promise.all(
        cacheNames.map((cacheName) => {
          if (cacheName !== CACHE_NAME && cacheName !== API_CACHE_NAME) {
            console.log('[ServiceWorker] Removing old cache:', cacheName);
            return caches.delete(cacheName);
          }
        })
      );
    })
  );
  // Take control of all clients immediately
  self.clients.claim();
});

// Fetch event - implement caching strategies
self.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);

  // Skip non-GET requests
  if (request.method !== 'GET') {
    return;
  }

  // Skip cross-origin requests
  if (url.origin !== self.location.origin) {
    return;
  }

  // Check if this is an API request that should skip cache
  if (SKIP_CACHE_PATTERNS.some(pattern => pattern.test(url.pathname))) {
    return; // Let the browser handle it normally
  }

  // Check if this is a cacheable API request
  const isAPI = url.pathname.startsWith('/api/');
  const isCacheableAPI = CACHEABLE_API_PATTERNS.some(pattern => pattern.test(url.pathname));

  if (isAPI && isCacheableAPI) {
    // Network-first strategy for API with fallback to cache
    event.respondWith(
      fetch(request)
        .then((response) => {
          // Cache successful responses
          if (response.ok) {
            const responseToCache = response.clone();
            caches.open(API_CACHE_NAME).then((cache) => {
              cache.put(request, responseToCache);
              // Store timestamp for cache expiration
              const cacheMetadata = {
                url: request.url,
                timestamp: Date.now()
              };
              cache.put(
                new Request(`${request.url}:metadata`),
                new Response(JSON.stringify(cacheMetadata))
              );
            });
          }
          return response;
        })
        .catch(() => {
          // Network failed, try cache
          return caches.match(request).then((cachedResponse) => {
            if (cachedResponse) {
              // Add custom header to indicate cached response
              const headers = new Headers(cachedResponse.headers);
              headers.set('X-From-Service-Worker-Cache', 'true');
              return new Response(cachedResponse.body, {
                status: cachedResponse.status,
                statusText: cachedResponse.statusText,
                headers: headers,
              });
            }
            // No cache available
            return new Response(
              JSON.stringify({ error: 'Offline - no cached data available' }),
              {
                status: 503,
                headers: { 'Content-Type': 'application/json' },
              }
            );
          });
        })
    );
  } else if (!isAPI) {
    // Cache-first strategy for static assets
    event.respondWith(
      caches.match(request).then((cachedResponse) => {
        if (cachedResponse) {
          // Update cache in background
          fetch(request).then((response) => {
            if (response.ok) {
              caches.open(CACHE_NAME).then((cache) => {
                cache.put(request, response);
              });
            }
          });
          return cachedResponse;
        }
        // Not in cache, fetch from network
        return fetch(request).then((response) => {
          // Cache successful responses
          if (response.ok && !isAPI) {
            const responseToCache = response.clone();
            caches.open(CACHE_NAME).then((cache) => {
              cache.put(request, responseToCache);
            });
          }
          return response;
        });
      })
    );
  }
});

// Listen for messages from the main thread
self.addEventListener('message', (event) => {
  if (event.data && event.data.type === 'SKIP_WAITING') {
    self.skipWaiting();
  }
  
  if (event.data && event.data.type === 'CLEAR_CACHE') {
    caches.keys().then((cacheNames) => {
      Promise.all(
        cacheNames.map((cacheName) => caches.delete(cacheName))
      ).then(() => {
        event.ports[0].postMessage({ success: true });
      });
    });
  }
});

// Periodic cache cleanup (every hour)
setInterval(() => {
  caches.open(API_CACHE_NAME).then((cache) => {
    cache.keys().then((requests) => {
      console.log(`[ServiceWorker] API cache has ${requests.length} entries`);
      // Could implement TTL-based cleanup here if needed
    });
  });
}, 60 * 60 * 1000);