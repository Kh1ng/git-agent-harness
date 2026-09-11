/*
 * Issue #534: GAH dashboard service worker — installable PWA shell.
 *
 * Cache policy (deliberately minimal, per the acceptance criteria):
 * - Static build assets (JS/CSS/fonts/icons) are cached versioned by the
 *   build's asset hashes — a cache-first strategy scoped to `/_asset/`.
 * - NOTHING under `/api/` is ever cached: ledger, config, session output,
 *   and auth responses must never survive into an offline or shared cache.
 *   Offline API calls simply fail and the UI shows its existing error +
 *   last-updated states (read-only, clearly stale).
 * - Navigation requests fall back to the cached shell so the app opens
 *   offline; queued mutations are forbidden by design — the offline app is
 *   read-only because every mutation is a network POST that fails closed.
 * - Update flow: when a new deploy changes the precache list, the worker
 *   skipWaiting()s and posts a message; the app shows an "update available"
 *   prompt and reloads on confirm (see src/pwa.ts).
 */

const CACHE_VERSION = 'gah-shell-v1';
const SHELL_ASSETS = ['/', '/manifest.webmanifest', '/icon-192.png', '/icon-512.png', '/favicon-32.png'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE_VERSION)
      .then((cache) => cache.addAll(SHELL_ASSETS))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE_VERSION).map((key) => caches.delete(key))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('message', (event) => {
  if (event.data === 'skip-waiting') self.skipWaiting();
});

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET') return;
  // Never touch API traffic — no cache, no fallback, no offline replay.
  if (url.pathname.startsWith('/api/')) return;

  if (event.request.mode === 'navigate') {
    event.respondWith(
      fetch(event.request)
        .then((response) => {
          const copy = response.clone();
          caches.open(CACHE_VERSION).then((cache) => cache.put('/', copy));
          return response;
        })
        .catch(() => caches.match('/').then((cached) => cached ?? Response.error()))
    );
    return;
  }

  // Hashed build assets: cache-first (a changed deploy produces new names).
  if (url.pathname.startsWith('/assets/')) {
    event.respondWith(
      caches.match(event.request).then(
        (cached) =>
          cached ??
          fetch(event.request).then((response) => {
            const copy = response.clone();
            caches.open(CACHE_VERSION).then((cache) => cache.put(event.request, copy));
            return response;
          })
      )
    );
    return;
  }

  // Other same-origin statics (icons, manifest): stale-while-revalidate.
  event.respondWith(
    caches.match(event.request).then((cached) => {
      const network = fetch(event.request)
        .then((response) => {
          const copy = response.clone();
          caches.open(CACHE_VERSION).then((cache) => cache.put(event.request, copy));
          return response;
        })
        .catch(() => cached ?? Response.error());
      return cached ?? network;
    })
  );
});
