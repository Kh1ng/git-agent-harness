import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware, isTrustedLocalRequest, requireOwner, sameOriginRequest } from './authMiddleware.js';
import { DEVICE_COOKIE, DEVICE_LIFETIME, type DeviceAccess } from './deviceAccess.js';
import type { CoordinatorIdentity } from './coordinatorIdentity.js';
import { browserOriginAllowed } from './webSocketAuth.js';

/** The only pre-auth API exception: inspect/redeem a one-time offer. Every
 * operation keeps transport/origin checks; managing offers/devices is owner-only. */
export function pairingRouter(access: DeviceAccess, identity: CoordinatorIdentity): Router {
  const router = Router();
  router.use((_req, res, next) => { res.set('Cache-Control', 'no-store'); next(); });
  router.use(rateLimit({ windowMs: 60_000, limit: 30, standardHeaders: true, legacyHeaders: false,
    message: { message: 'Too many pairing requests. Retry in a minute.' } }));
  router.post(['/inspect', '/redeem', '/logout'], (req, res) => {
    if ((!req.secure && !isTrustedLocalRequest(req) && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') || !sameOriginRequest(req)) {
      return res.status(403).json({ message: 'Pairing requires the same server origin and TLS, unless trusted HTTP is explicitly enabled.' });
    }
    const cookieOptions = { httpOnly: true, sameSite: 'strict' as const, secure: req.secure, path: '/' };
    if (req.path === '/logout') { res.clearCookie(DEVICE_COOKIE, cookieOptions); return res.json({ schema_version: 1, success: true }); }
    try {
      const input = req.body;
      const allowed = req.path === '/redeem' ? ['code', 'server_id', 'name', 'confirm'] : ['code', 'server_id'];
      if (!input || Object.keys(input).some(key => !allowed.includes(key))) throw new Error('Use a pairing code and the server identity shown by the owner.');
      const origin = new URL(`${req.protocol}://${req.headers.host}`).origin;
      if (req.path === '/inspect') return res.json(access.inspect(input.code, input.server_id, origin));
      if (input.confirm !== true) throw new Error('Confirm the server and requested access before pairing.');
      const { device, token } = access.redeem(input.code, input.server_id, origin, input.name);
      res.cookie(DEVICE_COOKIE, token, { ...cookieOptions, maxAge: DEVICE_LIFETIME });
      return res.json({ schema_version: 1, device });
    } catch (error) {
      return res.status(400).json({ message: error instanceof Error ? error.message : 'Cannot pair this device.' });
    }
  });
  router.use(authMiddleware);
  router.get('/session', (_req, res) => res.json({ schema_version: 1, principal: res.locals.authPrincipal }));
  router.use(requireOwner);
  router.get('/devices', (_req, res) => {
    try { res.json({ schema_version: 1, devices: access.list() }); }
    catch { res.status(503).json({ message: 'Cannot read paired devices.' }); }
  });
  router.post('/offers', (req, res) => {
    try {
      if (!req.body || Object.keys(req.body).some(key => key !== 'origin') || typeof req.body.origin !== 'string') throw new Error('Enter the central server address.');
      const origin = new URL(req.body.origin);
      if (!['http:', 'https:'].includes(origin.protocol) || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) throw new Error('Use an HTTP(S) server address without credentials or a path.');
      if (origin.protocol !== 'https:' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') throw new Error('Use HTTPS for pairing, or explicitly enable trusted HTTP on the server.');
      if (!browserOriginAllowed({ headers: { host: origin.host, origin: origin.origin } }, origin.protocol.slice(0, -1))) throw new Error('Allow this named server origin in GAH_WS_ALLOWED_ORIGINS before pairing.');
      res.json(access.create({ id: identity.node_id, name: identity.display_name, origin: origin.origin }));
    } catch (error) { res.status(400).json({ message: error instanceof Error ? error.message : 'Cannot create a pairing code.' }); }
  });
  router.delete('/devices/:id', (req, res) => {
    try { access.revoke(req.params.id); res.json({ schema_version: 1, success: true }); }
    catch { res.status(400).json({ message: 'Cannot revoke this device.' }); }
  });
  return router;
}
