/*
 * Issue #534: PWA plumbing — service-worker registration, update prompt,
 * and offline state. The service worker itself (public/sw.js) enforces the
 * cache policy: static shell only, /api/* never cached, offline read-only.
 */

/// Whether the browser is currently offline. Drives the stale banner.
export function isOffline(): boolean {
  return typeof navigator !== 'undefined' && navigator.onLine === false;
}

export function listenOfflineChange(onChange: (offline: boolean) => void): () => void {
  const goOffline = () => onChange(true);
  const goOnline = () => onChange(false);
  window.addEventListener('offline', goOffline);
  window.addEventListener('online', goOnline);
  return () => {
    window.removeEventListener('offline', goOffline);
    window.removeEventListener('online', goOnline);
  };
}

/// Register the service worker and surface a pending update. The caller
/// renders an "update available — reload" prompt when the callback fires and
/// calls the returned `apply` to activate it. No-op where SW is unsupported
/// (or on non-https origins other than localhost, per the register spec).
export function registerServiceWorker(onUpdateReady: (apply: () => void) => void): void {
  if (!('serviceWorker' in navigator)) return;
  if (location.protocol !== 'https:' && location.hostname !== 'localhost' && location.hostname !== '127.0.0.1') {
    // Service workers require a secure context; plain-HTTP LAN deployments
    // simply get no offline shell (the dashboard keeps working as before).
    return;
  }
  window.addEventListener('load', () => {
    navigator.serviceWorker
      .register('/sw.js')
      .then((registration) => {
        // If a waiting worker exists at registration time (deploy happened
        // while the tab was closed), surface it immediately.
        if (registration.waiting) {
          notifyUpdate(registration.waiting, onUpdateReady);
          return;
        }
        registration.addEventListener('updatefound', () => {
          const installing = registration.installing;
          if (!installing) return;
          installing.addEventListener('statechange', () => {
            if (installing.state === 'installed' && navigator.serviceWorker.controller) {
              notifyUpdate(installing, onUpdateReady);
            }
          });
        });
      })
      .catch(() => {
        // Registration failures are non-fatal; the dashboard is a normal
        // website without the offline shell.
      });
  });
}

function notifyUpdate(
  worker: ServiceWorker,
  onUpdateReady: (apply: () => void) => void
): void {
  onUpdateReady(() => {
    worker.postMessage('skip-waiting');
    navigator.serviceWorker.addEventListener('controllerchange', () => window.location.reload(), {
      once: true,
    });
  });
}
