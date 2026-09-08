import type { Request, Response, NextFunction } from 'express';
import crypto from 'crypto';
import { isIP } from 'node:net';

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
function isTrustedLocalRequest(req: Request): boolean {
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
  if (isTrustedLocalRequest(req)) {
    return next();
  }

  // Requests outside the local exemption require TLS and authenticated identity
  // Rely on Express's req.secure, which only trusts proxy headers if 'trust proxy' is configured.
  const isTls = req.secure;

  if (!isTls && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') {
    return res.status(403).json({
      error: 'Forbidden',
      message: 'Remote or cross-origin access requires TLS unless GAH_ALLOW_INSECURE_HTTP=1'
    });
  }

  // Authenticated node/client identity: check Bearer token
  const authHeader = req.headers.authorization;
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

  next();
}
