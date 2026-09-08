const TOKEN_KEY = 'gah.coordinatorToken';
export const TOKEN_CHANGED_EVENT = 'gah.coordinatorTokenChanged';

export function coordinatorToken(): string {
  try { return typeof window === 'undefined' ? '' : window.sessionStorage.getItem(TOKEN_KEY) ?? ''; }
  catch { return ''; }
}

/** Keep REST and live connections on the same tab-scoped credential. */
export function saveCoordinatorToken(token: string): void {
  if (token) window.sessionStorage.setItem(TOKEN_KEY, token);
  else window.sessionStorage.removeItem(TOKEN_KEY);
  window.dispatchEvent(new Event(TOKEN_CHANGED_EVENT));
}

/** Keep credentials out of request URLs; the server echoes only gah.v1. */
export function coordinatorWebSocketProtocols(): string[] {
  const token = coordinatorToken();
  if (!token) return ['gah.v1'];
  const encoded = btoa(Array.from(new TextEncoder().encode(token), byte => String.fromCharCode(byte)).join(''))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return ['gah.v1', `gah-auth.${encoded}`];
}
