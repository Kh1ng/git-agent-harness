import type { IncomingHttpHeaders, IncomingMessage, Server } from 'node:http';
import { isIP } from 'node:net';
import { WebSocket, WebSocketServer } from 'ws';
import { coordinatorTokenMatches, isLocalAddress, isTrustedLocalRequest, sameOriginRequest } from './authMiddleware.js';
import { deviceCookie, type DeviceAccess } from './deviceAccess.js';

const compatibilityClients = new WeakSet<WebSocket>();
const compatibilityWarnings = new WeakSet<WebSocket>();
const deviceClients = new WeakMap<WebSocket, { access: DeviceAccess; id: string }>();

/** Check before dispatching each message, including messages already buffered
 * when revocation terminated the transport. Revocation never stops accepted work. */
export function webSocketAccessValid(ws: WebSocket): boolean {
  const device = deviceClients.get(ws);
  if (!device || device.access.active(device.id)) return true;
  ws.terminate();
  return false;
}

/** Compatibility browsers may manage local chat, but cannot invoke the fleet coordinator. */
export function requiresFleetAuthentication(ws: WebSocket, messageType: string): boolean {
  return compatibilityClients.has(ws) && messageType.startsWith('session.');
}

/** An explicit deployment mode, never inferred from permission to use plain HTTP. */
export function trustedLanWebSocketMode(ws: WebSocket): boolean {
  return compatibilityWarnings.has(ws);
}

function requestProtocol(req: IncomingMessage): string {
  const encrypted = 'encrypted' in req.socket && req.socket.encrypted;
  const proxyTls = isLocalAddress(req.socket.remoteAddress ?? '') && req.headers['x-forwarded-proto'] === 'https';
  return encrypted || proxyTls ? 'https' : 'http';
}

/** Browser origins must target a literal/local address or an explicitly configured origin.
 * Host equality alone would let an attacker's domain rebind to the local server. */
export function browserOriginAllowed(req: { headers: IncomingHttpHeaders }, protocol: string): boolean {
  try {
    if (req.headers.origin === undefined) return true;
    const target = new URL(`${protocol}://${req.headers.host}`);
    const configured = (process.env.GAH_WS_ALLOWED_ORIGINS ?? '').split(',').filter(Boolean).map(origin => new URL(origin.trim()).origin);
    const hostname = target.hostname.replace(/^\[|\]$/g, '');
    if (!isIP(hostname) && !isLocalAddress(hostname) && !configured.includes(target.origin)) return false;
    const origin = new URL(req.headers.origin);
    return ['http:', 'https:'].includes(origin.protocol) && (origin.origin === target.origin || configured.includes(origin.origin));
  } catch {
    return false;
  }
}

function browserToken(req: IncomingMessage): string | null {
  const protocols = (req.headers['sec-websocket-protocol'] ?? '').split(',').map(value => value.trim());
  const credentials = protocols.filter(value => value.startsWith('gah-auth.'));
  if (credentials.length !== 1 || !protocols.includes('gah.v1')) return null;
  const encoded = credentials[0].slice('gah-auth.'.length);
  if (!/^[A-Za-z0-9_-]+$/.test(encoded)) return null;
  const decoded = Buffer.from(encoded, 'base64url');
  return decoded.toString('base64url') === encoded ? decoded.toString('utf8') : null;
}

/** Reject unauthorized upgrades before a socket can receive welcome data or invoke handlers.
 * Native fleet clients retain Authorization; browsers send a non-echoed credential protocol. */
export function createAuthorizedWebSocketServer(server: Server, role?: 'central' | 'worker', access?: DeviceAccess): WebSocketServer {
  const compatibilityRequests = new WeakSet<IncomingMessage>();
  const deviceRequests = new WeakMap<IncomingMessage, { id: string; expires_at: string }>();
  const wss = new WebSocketServer({
    server,
    handleProtocols: protocols => protocols.has('gah.v1') ? 'gah.v1' : false,
    verifyClient: ({ req }, done) => {
      const protocol = requestProtocol(req);
      if (!browserOriginAllowed(req, protocol)) return done(false, 403, 'WebSocket origin or host is not allowed');
      const worker = (role ?? process.env.GAH_NODE_ROLE) === 'worker';
      const local = isTrustedLocalRequest({ socket: req.socket, headers: req.headers, protocol });
      if (!local && protocol !== 'https' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1') return done(false, 403, 'Remote WebSockets require TLS');
      const authorization = req.headers.authorization;
      const token = authorization?.startsWith('Bearer ') ? authorization.slice(7) : browserToken(req);
      if (token && coordinatorTokenMatches(token)) return done(true);
      const suppliedToken = authorization !== undefined || req.headers['sec-websocket-protocol']?.includes('gah-auth.');
      const cookie = deviceCookie(req.headers.cookie);
      if (!suppliedToken && cookie !== undefined) {
        try {
          const device = !worker && sameOriginRequest({ headers: req.headers, protocol, method: req.method }) ? access?.authenticate(cookie) : null;
          if (device) { deviceRequests.set(req, device); return done(true); }
        } catch { /* Corrupt credential storage must not authorize an upgrade. */ }
        return done(false, 401, 'Device access expired or revoked');
      }
      if (!worker && local && !suppliedToken) return done(true);
      // A bad supplied credential is not silently downgraded into compatibility mode.
      if (token === null && !suppliedToken && cookie === undefined && req.headers.origin && !worker && process.env.GAH_WS_AUTH_MODE === 'trusted_lan') {
        compatibilityRequests.add(req);
        return done(true);
      }
      return done(false, 401, 'Coordinator token required');
    }
  });
  wss.on('connection', (ws, req) => {
    const device = deviceRequests.get(req);
    if (device && access) {
      deviceClients.set(ws, { access, id: device.id });
      const unsubscribe = access.onRevoke(id => { if (id === device.id) ws.terminate(); });
      let expiry: ReturnType<typeof setTimeout>;
      const expire = () => {
        const remaining = Date.parse(device.expires_at) - Date.now();
        if (remaining <= 0) { ws.terminate(); return; }
        expiry = setTimeout(expire, Math.min(remaining, 2_147_483_647));
        expiry.unref();
      };
      expire();
      ws.once('close', () => { clearTimeout(expiry); unsubscribe(); });
    }
    if (compatibilityRequests.has(req)) compatibilityClients.add(ws);
    if ((role ?? process.env.GAH_NODE_ROLE) !== 'worker' && process.env.GAH_WS_AUTH_MODE === 'trusted_lan') compatibilityWarnings.add(ws);
  });
  return wss;
}
