import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware, isTrustedLocalRequest, requireOwner, sameOriginRequest } from './authMiddleware.js';
import { DEVICE_COOKIE, DEVICE_LIFETIME, PAIRING_REQUEST_COOKIE, credentialCookie, type DeviceAccess } from './deviceAccess.js';
import type { CoordinatorIdentity } from './coordinatorIdentity.js';
import { browserOriginAllowed } from './webSocketAuth.js';
import type { PairingAccessRequest } from '@git-agent-harness/contracts';

/** Public offers and approval requests keep transport/origin checks. Owners
 * manage devices and offers; explicitly delegated controllers may review sign-ins. */
export function pairingRouter(access: DeviceAccess, identity: CoordinatorIdentity, requested?: (request: PairingAccessRequest) => void): Router {
  const router = Router();
  router.use((_req, res, next) => { res.set('Cache-Control', 'no-store'); next(); });
  router.use(rateLimit({ windowMs: 60_000, limit: 30, standardHeaders: true, legacyHeaders: false,
    skip: req => req.method === 'GET' && ['/session', '/access/status', '/access/requests'].includes(req.path),
    message: { message: 'Too many pairing requests. Retry in a minute.' } }));
  router.get(['/session', '/access/status', '/access/requests'], rateLimit({ windowMs: 60_000, limit: 120, standardHeaders: true, legacyHeaders: false,
    message: { message: 'Too many approval status checks. Retry in a minute.' } }));
  router.all(['/access/request', '/access/status', '/access/claim'], (req, res, next) => {
    if ((!req.secure && !isTrustedLocalRequest(req)) || !sameOriginRequest(req)) {
      return res.status(403).json({ message: 'Device approval requires the same server origin and TLS.' });
    }
    next();
  });
  router.post('/access/request', (req, res) => {
    try {
      if (!req.body || Object.keys(req.body).some(key => key !== 'name')) throw new Error('Enter the requesting device name.');
      const origin = new URL(`${req.protocol}://${req.headers.host}`).origin;
      const result = access.createAccessRequest({ id: identity.node_id, name: identity.display_name, origin }, req.body.name);
      requested?.(result.request);
      res.cookie(PAIRING_REQUEST_COOKIE, result.cookie, { httpOnly: true, sameSite: 'strict', secure: true, path: '/api/pairing/access', maxAge: 5 * 60_000 });
      res.status(202).json(result.request);
    } catch (error) { res.status(400).json({ message: error instanceof Error ? error.message : 'Cannot request device access.' }); }
  });
  router.get('/access/status', (req, res) => {
    try {
      const origin = new URL(`${req.protocol}://${req.headers.host}`).origin;
      res.json(access.accessRequestStatus(credentialCookie(req.headers.cookie, PAIRING_REQUEST_COOKIE), origin));
    } catch { res.status(404).json({ message: 'No access request is bound to this browser.' }); }
  });
  router.post('/access/claim', (req, res) => {
    try {
      if (!req.body || req.body.confirm !== true || Object.keys(req.body).some(key => key !== 'confirm')) throw new Error('Confirm this server and controller access before continuing.');
      const origin = new URL(`${req.protocol}://${req.headers.host}`).origin;
      const paired = access.claimAccessRequest(credentialCookie(req.headers.cookie, PAIRING_REQUEST_COOKIE), origin);
      const options = { httpOnly: true, sameSite: 'strict' as const, secure: true };
      res.cookie(DEVICE_COOKIE, paired.token, { ...options, path: '/', maxAge: DEVICE_LIFETIME });
      res.clearCookie(PAIRING_REQUEST_COOKIE, { ...options, path: '/api/pairing/access' });
      res.json({ schema_version: 1, device: paired.device });
    } catch (error) { res.status(400).json({ message: error instanceof Error ? error.message : 'Cannot claim device access.' }); }
  });
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
  router.get('/session', (_req, res) => res.json({ schema_version: 1, principal: res.locals.authPrincipal,
    can_approve_pairing: res.locals.authPrincipal?.kind === 'owner' || access.canApprovePairing(res.locals.authPrincipal?.id ?? '') }));
  router.use('/access/requests', (_req, res, next) => {
    if (res.locals.authPrincipal?.kind === 'owner' || access.canApprovePairing(res.locals.authPrincipal?.id ?? '')) return next();
    res.status(403).json({ message: 'Pairing approval is not enabled for this device.' });
  });
  router.get('/access/requests', (_req, res) => res.json({ schema_version: 1, requests: access.accessRequests() }));
  router.post('/access/requests/:id/:decision', (req, res) => {
    if (!sameOriginRequest(req)) return res.status(403).json({ message: 'Review access requests from the same server origin.' });
    const approve = req.params.decision === 'approve';
    if (!approve && req.params.decision !== 'deny') return res.status(400).json({ message: 'Choose approve or deny.' });
    const allowed = approve ? ['matching_code', 'confirm'] : [];
    if (!req.body || Object.keys(req.body).some(key => !allowed.includes(key)) || (approve && req.body.confirm !== true)) return res.status(400).json({ message: 'Confirm the matching code before approving.' });
    try {
      res.json(access.decideAccessRequest(req.params.id, approve, req.body.matching_code,
        res.locals.authPrincipal.kind === 'owner' ? 'owner' : res.locals.authPrincipal.id));
    } catch (error) { res.status(409).json({ message: error instanceof Error ? error.message : 'Cannot review access request.' }); }
  });
  router.use(requireOwner);
  router.patch('/devices/:id', (req, res) => {
    if (!sameOriginRequest(req)) return res.status(403).json({ message: 'Change pairing approval from the same server origin.' });
    if (!req.body || Object.keys(req.body).some(key => key !== 'can_approve_pairing') || typeof req.body.can_approve_pairing !== 'boolean') return res.status(400).json({ message: 'Supply a boolean pairing approval setting.' });
    try { res.json({ schema_version: 1, device: access.setCanApprovePairing(req.params.id, req.body.can_approve_pairing) }); }
    catch { res.status(400).json({ message: 'Cannot change pairing approval for this device.' }); }
  });
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
