import type { Request, Response, NextFunction } from 'express';
import crypto from 'crypto';

export function isLocalAddress(ip: string): boolean {
  if (!ip) return false;
  return (
    ip === '127.0.0.1' ||
    ip === '::1' ||
    ip === '::ffff:127.0.0.1' ||
    ip.startsWith('127.') ||
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

function isLoopbackRequest(req: Request): boolean {
  // Trust the TCP socket address only, not req.ip (which with trust proxy:loopback
  // reflects X-Forwarded-For, making Caddy-proxied LAN requests look non-local).
  // The socket source is tamper-proof; X-Forwarded-For spoofing is already blocked
  // by Express only honoring it when the socket itself comes from loopback.
  const socketAddress = req.socket.remoteAddress || '';
  return isLocalAddress(socketAddress);
}

export function authMiddleware(req: Request, res: Response, next: NextFunction) {
  // Only skip auth for direct loopback requests. If a proxy sits in front of
  // the server, forwarded headers must not be enough to claim localhost access.
  if (isLoopbackRequest(req)) {
    return next();
  }

  // Non-loopback endpoints require TLS plus authenticated node/client identity
  // Rely on Express's req.secure, which only trusts proxy headers if 'trust proxy' is configured.
  const isTls = req.secure;

  if (!isTls && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') {
    return res.status(403).json({
      error: 'Forbidden',
      message: 'Non-loopback endpoints require TLS unless GAH_ALLOW_INSECURE_HTTP=1'
    });
  }

  // Authenticated node/client identity: check Bearer token
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    return res.status(401).json({
      error: 'Unauthorized',
      message: 'Authentication token required for non-loopback access'
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
