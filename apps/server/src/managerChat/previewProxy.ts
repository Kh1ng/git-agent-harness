/**
 * Session preview proxy (WP3 environments): one dedicated listen port per
 * chat session, proxying HTTP + WebSocket traffic to the dev server the
 * agent started inside the session's worktree (127.0.0.1:<devPort>).
 *
 * Why a dedicated port instead of a path prefix (/preview/<session>/...):
 * dev servers emit absolute asset URLs (/@vite/client, /src/main.tsx), so
 * prefix-stripping proxies 404 on the first reload. A real port per session
 * needs no rewriting at all.
 *
 * The proxied request's Host header is rewritten to `localhost:<devPort>`:
 * Vite (5.4+) and friends reject foreign Hosts as DNS-rebind protection,
 * and the app inside the worktree only ever expects localhost.
 *
 * Listen ports come from a small dedicated range (default 41000-41099) so
 * the operator can firewall exactly the preview surface. The advertised URL
 * uses the node's Tailscale IPv4 when present (WireGuard-encrypted in
 * transit), else the loopback for local dev.
 */

import http from 'node:http';
import net from 'node:net';
import { detectTailscaleIPv4 } from '../tailscaleDetect.js';

export interface PreviewInfo {
  profile: string;
  sessionId: string;
  /** Port the dev server listens on inside the node (127.0.0.1). */
  devPort: number;
  /** Dedicated port this proxy listens on for the session. */
  listenPort: number;
  /** Browser-facing URL (tailscale IP when available). */
  url: string;
}

interface PreviewEntry extends PreviewInfo {
  server: http.Server;
}

export interface PreviewProxyOptions {
  /** First port of the listen range (tests override). */
  basePort?: number;
  /** Inclusive last port of the listen range. */
  maxPort?: number;
  /** Advertised host in preview URLs (tests override). */
  advertiseHost?: string;
}

const DEFAULT_BASE_PORT = 41_000;
const DEFAULT_MAX_PORT = 41_099;

class PreviewProxy {
  private byKey = new Map<string, PreviewEntry>();
  /** In-flight set() per key: concurrent detections (tool summary AND tool
   * result in the same turn) must not each create a server -- the second
   * would leak (the existence check runs before the first registers). */
  private inflightSets = new Map<string, Promise<PreviewInfo>>();
  private options: Required<Pick<PreviewProxyOptions, 'basePort' | 'maxPort'>> & { advertiseHost?: string } = {
    basePort: Number.parseInt(process.env.GAH_PREVIEW_BASE_PORT ?? '', 10) || DEFAULT_BASE_PORT,
    maxPort: Number.parseInt(process.env.GAH_PREVIEW_MAX_PORT ?? '', 10) || DEFAULT_MAX_PORT
  };

  configure(opts: PreviewProxyOptions): void {
    if (opts.basePort !== undefined) this.options.basePort = opts.basePort;
    if (opts.maxPort !== undefined) this.options.maxPort = opts.maxPort;
    if (opts.advertiseHost !== undefined) this.options.advertiseHost = opts.advertiseHost;
  }

  get(profile: string, sessionId: string): PreviewInfo | null {
    const entry = this.byKey.get(`${profile}#${sessionId}`);
    if (!entry) return null;
    const { server: _server, ...info } = entry;
    return info;
  }

  /** Lists all live previews (dashboard/debug). */
  list(): PreviewInfo[] {
    return [...this.byKey.values()].map(({ server: _s, ...info }) => info);
  }

  /** Points (or re-points) a session's preview at a dev port. The listen
   * port is STABLE for the session's lifetime — a dev-server restart on a
   * new port keeps the same preview URL; the proxy resolves the current
   * target per request. */
  async set(profile: string, sessionId: string, devPort: number): Promise<PreviewInfo> {
    const key = `${profile}#${sessionId}`;
    // Serialize per key: a concurrent set waits for the in-flight one, then
    // just re-points the (now existing) entry instead of racing a second
    // server into existence.
    const inflight = this.inflightSets.get(key);
    if (inflight) await inflight.catch(() => undefined);
    const existing = this.byKey.get(key);
    if (existing) {
      existing.devPort = devPort;
      const { server: _server, ...info } = existing;
      return info;
    }

    const op = this.createEntry(key, profile, sessionId, devPort);
    this.inflightSets.set(key, op);
    try {
      return await op;
    } finally {
      this.inflightSets.delete(key);
    }
  }

  private async createEntry(key: string, profile: string, sessionId: string, devPort: number): Promise<PreviewInfo> {
    // The entry is created before its server so the proxy handlers resolve
    // the CURRENT devPort per request (re-points hit the same listener).
    const entry = {
      profile,
      sessionId,
      devPort,
      listenPort: 0,
      url: '',
      server: undefined as unknown as http.Server
    } as PreviewEntry;
    entry.server = this.createProxyServer(() => entry.devPort);
    const listenPort = await this.listenOnFreePort(entry.server);
    const host = this.options.advertiseHost ?? (await detectTailscaleIPv4()) ?? '127.0.0.1';
    entry.listenPort = listenPort;
    entry.url = `http://${host}:${listenPort}`;
    this.byKey.set(key, entry);
    const { server: _server, ...info } = entry;
    return info;
  }

  async clear(profile: string, sessionId: string): Promise<void> {
    const key = `${profile}#${sessionId}`;
    // Wait out any in-flight set first, or it would register a fresh entry
    // (and listener) right after this clear removed the old one.
    const inflight = this.inflightSets.get(key);
    if (inflight) await inflight.catch(() => undefined);
    const entry = this.byKey.get(key);
    if (!entry) return;
    await this.closeEntry(entry, key);
  }

  async closeAll(): Promise<void> {
    for (const [key, entry] of [...this.byKey.entries()]) {
      await this.closeEntry(entry, key);
    }
  }

  private async closeEntry(entry: PreviewEntry, key: string): Promise<void> {
    this.byKey.delete(key);
    await new Promise<void>((resolve) => {
      entry.server.close(() => resolve());
      // close() waits for open connections; force-destroy idle keep-alive
      // sockets so a closed preview actually releases its port.
      entry.server.closeAllConnections?.();
      entry.server.closeIdleConnections?.();
    });
  }

  private createProxyServer(getDevPort: () => number): http.Server {
    const server = http.createServer((req, res) => {
      const devPort = getDevPort();
      // agent:false — one connection per request. The global agent pools
      // keep-alive sockets to the dev server that outlive preview clears
      // (and pin the process open in tests); preview traffic doesn't need
      // the reuse.
      const proxied = http.request(
        {
          host: '127.0.0.1',
          port: devPort,
          method: req.method,
          path: req.url,
          headers: { ...req.headers, host: `localhost:${devPort}` },
          agent: false
        },
        (upstream) => {
          res.writeHead(upstream.statusCode ?? 502, upstream.headers);
          upstream.pipe(res);
        }
      );
      proxied.on('error', () => {
        if (!res.headersSent) res.writeHead(502, { 'content-type': 'text/plain' });
        res.end(`preview target unreachable: is the dev server running on port ${devPort}?`);
      });
      req.pipe(proxied);
    });

    // WebSocket upgrade (Vite HMR): splice a raw TCP socket to the dev
    // server with the Host header rewritten. No frame inspection needed.
    server.on('upgrade', (req, socket, head) => {
      const devPort = getDevPort();
      const upstream = net.connect(devPort, '127.0.0.1', () => {
        const lines = [`${req.method} ${req.url} HTTP/${req.httpVersion}`];
        for (const [name, value] of Object.entries(req.headers)) {
          if (name === 'host') {
            lines.push(`host: localhost:${devPort}`);
          } else if (Array.isArray(value)) {
            for (const v of value) lines.push(`${name}: ${v}`);
          } else if (value !== undefined) {
            lines.push(`${name}: ${value}`);
          }
        }
        upstream.write(`${lines.join('\r\n')}\r\n\r\n`);
        if (head.length > 0) upstream.write(head);
        socket.pipe(upstream);
        upstream.pipe(socket);
      });
      upstream.on('error', () => socket.destroy());
      socket.on('error', () => upstream.destroy());
    });

    return server;
  }

  private listenOnFreePort(server: http.Server): Promise<number> {
    const candidates: number[] = [];
    const used = new Set([...this.byKey.values()].map((e) => e.listenPort));
    for (let port = this.options.basePort; port <= this.options.maxPort; port++) {
      if (!used.has(port)) candidates.push(port);
    }
    if (candidates.length === 0) {
      return Promise.reject(new Error('preview port range exhausted'));
    }
    return new Promise<number>((resolve, reject) => {
      const tryNext = (index: number): void => {
        if (index >= candidates.length) {
          reject(new Error('no free port in the preview range'));
          return;
        }
        const port = candidates[index];
        server.once('error', () => tryNext(index + 1));
        server.listen(port, '0.0.0.0', () => resolve(port));
      };
      tryNext(0);
    });
  }
}

export const previewProxy = new PreviewProxy();

/**
 * Auto-detects a dev-server port from agent tool output (the slice-3
 * structured stream). Matches the shapes the common dev servers print:
 * "Local: http://localhost:5173/", "listening on 127.0.0.1:3000",
 * "running at http://[::1]:8080", "Server running on port 4200".
 * Returns the LAST match (dev servers restart and print again).
 */
export function detectDevPort(text: string): number | null {
  const patterns = [
    /(?:localhost|127\.0\.0\.1|\[::1\]):(\d{2,5})/gi,
    /\bport\s+(\d{2,5})\b/gi
  ];
  let port: number | null = null;
  for (const pattern of patterns) {
    for (const match of text.matchAll(pattern)) {
      const candidate = Number.parseInt(match[1], 10);
      if (candidate >= 1024 && candidate <= 65_535) port = candidate;
    }
  }
  return port;
}
