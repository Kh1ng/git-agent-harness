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

function isLoopbackRequest(req: Request): boolean {
  const socketAddress = req.socket.remoteAddress || '';
  const clientIp = req.ip || socketAddress;
  return isLocalAddress(socketAddress) && isLocalAddress(clientIp);
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

  if (!isTls) {
    return res.status(403).json({
      error: 'Forbidden',
      message: 'Non-loopback endpoints require TLS'
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

  // Use constant-time comparison to prevent timing attacks
  const tokenHash = crypto.createHash('sha256').update(token).digest();
  const expectedHash = crypto.createHash('sha256').update(expectedToken).digest();
  const tokensMatch = crypto.timingSafeEqual(tokenHash, expectedHash);

  if (!tokensMatch) {
    return res.status(401).json({
      error: 'Unauthorized',
      message: 'Invalid authentication token'
    });
  }

  next();
}
