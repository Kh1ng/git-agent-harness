import { isIP } from 'node:net';

/** Issue #643: the application default stays 0.0.0.0 -- operators opt into a
 * narrower bind via HOST (direct process) or /etc/gah/server.env (systemd). */
export const DEFAULT_BIND_HOST = '0.0.0.0';

export function resolveBindHost(env: NodeJS.ProcessEnv = process.env): string {
  const value = env.HOST?.trim();
  return value ? value : DEFAULT_BIND_HOST;
}

export class InvalidBindHostError extends Error {}

/** A bind address must be a literal IP; reject hostnames/garbage before
 * `server.listen` turns a typo into a confusing ENOTFOUND/EADDRNOTAVAIL. */
export function validateBindHost(host: string): void {
  if (isIP(host) === 0) {
    throw new InvalidBindHostError(
      `Invalid HOST bind address "${host}": expected a literal IPv4 or IPv6 address ` +
        `(for example 0.0.0.0, 127.0.0.1, ::1, or a specific interface address).`
    );
  }
}

export function isLoopbackBindHost(host: string): boolean {
  return host === '127.0.0.1' || host === '::1' || host.startsWith('127.');
}

/** Remind operators of the remote authentication and network boundary when
 * the server listens beyond loopback. */
export function networkExposureWarning(host: string): string | null {
  if (isLoopbackBindHost(host)) {
    return null;
  }
  return (
    `WARNING: gah-server is bound to ${host}, which is reachable beyond this host. ` +
    `Remote API access requires COORDINATOR_TOKEN. TLS is required unless GAH_ALLOW_INSECURE_HTTP=1. ` +
    `GAH_WS_AUTH_MODE=trusted_lan permits limited unauthenticated WebSocket access. ` +
    `Restrict network access (firewall/VPN/Tailscale) or set HOST=127.0.0.1 ` +
    `in /etc/gah/server.env. See docs/WEBSOCKET_AUTH_532.md.`
  );
}
