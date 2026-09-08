import type { Request, Response, NextFunction } from 'express';
import crypto from 'crypto';
import { isIP } from 'node:net';
import type { IncomingHttpHeaders } from 'node:http';
import { deviceCookie, type DeviceAccess } from './deviceAccess.js';

export function isLocalAddress(ip: string): boolean {
  if (!ip) return false;
  return (
    ip === '::1' ||
    ip === '::ffff:127.0.0.1' ||
    (isIP(ip) === 4 && ip.startsWith('127.')) ||
    ip === 'localhost'
  );
}

/** Timing-safe check of a Bearer token against the configured coordinator
 * token. Returns false when the token is unset too, so callers that need to
 * distinguish "token not configured" can check `process.env.COORDINATOR_TOKEN`
 * themselves. */
export function coordinatorTokenMatches(token: string): boolean {
  const expected = process.env.COORDINATOR_TOKEN;
  if (!expected) return false;
  const tokenHash = crypto.createHash('sha256').update(token).digest();
  const expectedHash = crypto.createHash('sha256').update(expected).digest();
  return crypto.timingSafeEqual(tokenHash, expectedHash);
}

/** Local CLI and same-origin browser requests may omit the token. A proxy
 * hop or an unrelated browser origin never inherits the socket's local trust. */
export function isTrustedLocalRequest(req: { socket: { remoteAddress?: string }; headers: IncomingHttpHeaders; protocol: string }): boolean {
  if (!isLocalAddress(req.socket.remoteAddress || '')) return false;
  if (Object.keys(req.headers).some((name) => name === 'forwarded' || name.startsWith('x-forwarded-'))) return false;
  try {
    const target = new URL(`${req.protocol}://${req.headers.host}`);
    // Checking Host also rejects DNS rebinding, including GETs without Origin.
    if (!isLocalAddress(target.hostname.replace(/^\[|\]$/g, ''))) return false;
    const origin = req.headers.origin;
    return origin === undefined || new URL(origin).origin === target.origin;
  } catch {
    return false;
  }
}

export function authMiddleware(req: Request, res: Response, next: NextFunction) {
  const local = isTrustedLocalRequest(req);

  // Requests outside the local exemption require TLS and authenticated identity
  // Rely on Express's req.secure, which only trusts proxy headers if 'trust proxy' is configured.
  const isTls = req.secure;

  if (!local && !isTls && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') {
    return res.status(403).json({
      error: 'Forbidden',
      message: 'Remote or cross-origin access requires TLS unless GAH_ALLOW_INSECURE_HTTP=1'
    });
  }

  // Authenticated node/client identity: check Bearer token
  const authHeader = req.headers.authorization;
  const cookie = deviceCookie(req.headers.cookie);
  // A revoked/expired supplied device credential must not inherit local trust.
  // Explicit owner credentials can replace an old paired session in this browser.
  if (authHeader === undefined && cookie !== undefined) {
    try {
      const access: DeviceAccess | undefined = req.app?.locals.deviceAccess;
      const device = sameOriginRequest(req) ? access?.authenticate(cookie) : null;
      if (device) { res.locals.authPrincipal = { kind: 'device', id: device.id }; return next(); }
    } catch { /* Invalid/unreadable credential storage fails closed. */ }
    return res.status(401).json({ error: 'Unauthorized', message: 'Device access is unavailable, expired, or revoked. Pair this device again.' });
  }
  if (authHeader === undefined && local) {
    res.locals.authPrincipal = { kind: 'owner' };
    return next();
  }
  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    return res.status(401).json({
      error: 'Unauthorized',
      message: 'Authentication token required for remote or cross-origin access'
    });
  }

  const token = authHeader.substring(7);
  const expectedToken = process.env.COORDINATOR_TOKEN;

  if (!expectedToken) {
    return res.status(500).json({
      error: 'Internal Server Error',
      message: 'Coordinator authentication token is not configured on the server'
    });
  }

  if (!coordinatorTokenMatches(token)) {
    return res.status(401).json({
      error: 'Unauthorized',
      message: 'Invalid authentication token'
    });
  }

  res.locals.authPrincipal = { kind: 'owner' };
  next();
}

/** Cookie credentials are ambient: require the browser's exact origin, not a
 * same-site subdomain. HTTP browser reads can omit both Origin and Fetch
 * Metadata; their Referer must then prove the same origin. */
export function sameOriginRequest(req: { headers: IncomingHttpHeaders; protocol: string; method?: string }): boolean {
  try {
    const target = new URL(`${req.protocol}://${req.headers.host}`);
    if (req.headers.origin !== undefined) return new URL(req.headers.origin).origin === target.origin;
    if (req.method !== 'GET' && req.method !== 'HEAD') return false;
    if (req.headers['sec-fetch-site'] !== undefined) return req.headers['sec-fetch-site'] === 'same-origin';
    if (req.headers.upgrade !== undefined || req.headers.referer === undefined) return false;
    return new URL(req.headers.referer).origin === target.origin;
  } catch { return false; }
}

/** Paired devices may control work, but cannot change host trust, credentials,
 * global configuration, or perform destructive administration. */
export function requireOwner(_req: Request, res: Response, next: NextFunction) {
  if (res.locals.authPrincipal?.kind !== 'owner') return res.status(403).json({ error: 'Forbidden', message: 'This operation requires owner access.' });
  next();
}
