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

/** Mutation routes (profiles, config, loop start/stop) have no
 * authentication yet -- that's issue #532. Binding non-loopback makes this
 * host reachable to whoever can reach the interface, so surface it loudly
 * instead of leaving the exposure implicit. */
export function unauthenticatedExposureWarning(host: string): string | null {
  if (isLoopbackBindHost(host)) {
    return null;
  }
  return (
    `WARNING: gah-server is bound to ${host}, which is reachable beyond this host. ` +
    `The server has no built-in authentication yet (see issue #532), so anyone who can ` +
    `reach this address can call its mutating API routes (profile writes, config changes, ` +
    `loop start/stop). Restrict network access (firewall/VPN/Tailscale) or set HOST=127.0.0.1 ` +
    `in /etc/gah/server.env until #532 ships. See docs/OPERATIONS.md.`
  );
}
