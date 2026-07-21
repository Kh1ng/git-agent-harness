import type { Request, Response, NextFunction } from 'express';

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

export function authMiddleware(req: Request, res: Response, next: NextFunction) {
  // Check if request is loopback
  const clientIp = req.ip || req.socket.remoteAddress || '';
  if (isLocalAddress(clientIp)) {
    // Localhost development remains explicit
    return next();
  }

  // Non-loopback endpoints require TLS plus authenticated node/client identity
  // Trust proxy headers like x-forwarded-proto or check req.secure
  const isForwardedTls = req.headers['x-forwarded-proto'] === 'https';
  const isTls = req.secure || isForwardedTls;

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

  if (token !== expectedToken) {
    return res.status(401).json({
      error: 'Unauthorized',
      message: 'Invalid authentication token'
    });
  }

  next();
}
