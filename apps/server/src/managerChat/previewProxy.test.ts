import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { previewProxy, detectDevPort, PreviewTargetRefusedError } from './previewProxy.js';
import { authMiddleware } from '../authMiddleware.js';

const TEST_BASE_PORT = 44_900;
const TEST_MAX_PORT = 44_949;

/** listen(0) on an ephemeral port outside the preview range the tests
 * configure, which `set()` refuses as a target. */
async function listenOutsidePreviewRange(server: http.Server): Promise<number> {
  for (;;) {
    await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
    const port = (server.address() as AddressInfo).port;
    if (port < TEST_BASE_PORT || port > TEST_MAX_PORT) return port;
    await new Promise<void>((done) => server.close(() => done()));
  }
}

/** A fake dev server that records the Host header it was served with. */
async function fakeDevServer(handler: http.RequestListener): Promise<{ server: http.Server; port: number; close: () => Promise<void> }> {
  const server = http.createServer(handler);
  const port = await listenOutsidePreviewRange(server);
  return {
    server,
    port,
    close: () => new Promise<void>((done) => server.close(() => done()))
  };
}

function fetchUrl(url: string): Promise<{ status: number; body: string; headers: http.IncomingHttpHeaders }> {
  return new Promise((resolve, reject) => {
    // agent:false -- a fresh connection per fetch. The global agent pools
    // keep-alive sockets per host:port, and a pooled socket to a preview
    // port whose listener was closed+rebound mid-suite gets reused after
    // the server destroyed it (ECONNRESET). Browsers don't do this.
    http.get(url, { agent: false }, (res) => {
      let body = '';
      res.on('data', (chunk) => { body += chunk; });
      res.on('end', () => resolve({ status: res.statusCode ?? 0, body, headers: res.headers }));
    }).on('error', reject);
  });
}

test('detectDevPort matches common dev-server output shapes, last match wins', () => {
  assert.equal(detectDevPort('VITE ready\n  Local: http://localhost:5173/'), 5173);
  assert.equal(detectDevPort('listening on 127.0.0.1:3000'), 3000);
  assert.equal(detectDevPort('Server running at http://[::1]:8080'), 8080);
  assert.equal(detectDevPort('Server running on port 4200'), 4200);
  assert.equal(detectDevPort('restarting...\nLocal: http://localhost:3000/\nLocal: http://localhost:3001/'), 3001);
  assert.equal(detectDevPort('nothing here'), null);
  // Sanity bounds: system ports and nonsense are ignored.
  assert.equal(detectDevPort('localhost:80'), null);
});

test('preview proxy serves the dev server through a dedicated port with the Host rewritten to localhost', async () => {
  previewProxy.configure({ basePort: TEST_BASE_PORT, maxPort: TEST_MAX_PORT, advertiseHost: '127.0.0.1' });
  const seenHosts: string[] = [];
  const dev = await fakeDevServer((req, res) => {
    seenHosts.push(String(req.headers.host));
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<h1>preview</h1>');
  });
  try {
    const preview = await previewProxy.set('p', 's1', dev.port);
    assert.ok(preview.listenPort >= 44_900 && preview.listenPort <= 44_949, 'listen port from the configured range');
    assert.equal(preview.devPort, dev.port);
    assert.equal(preview.url, `http://127.0.0.1:${preview.listenPort}`);

    const served = await fetchUrl(preview.url);
    assert.equal(served.status, 200);
    assert.match(served.body, /preview/);
    assert.deepEqual(seenHosts, [`localhost:${dev.port}`], 'Host header rewritten (defeats dev-server allowedHosts checks)');

    // Same dev port set again is idempotent (same listener, no leak).
    const again = await previewProxy.set('p', 's1', dev.port);
    assert.equal(again.listenPort, preview.listenPort);

    await previewProxy.clear('p', 's1');
    assert.equal(previewProxy.get('p', 's1'), null);
  } finally {
    await previewProxy.clear('p', 's1');
    await dev.close();
  }
});

test('preview proxy re-points a session to a new dev port (stable URL) and reports unreachable targets as 502', async () => {
  previewProxy.configure({ basePort: TEST_BASE_PORT, maxPort: TEST_MAX_PORT, advertiseHost: '127.0.0.1' });
  const first = await fakeDevServer((_req, res) => { res.end('one'); });
  const second = await fakeDevServer((_req, res) => { res.end('two'); });
  try {
    const p1 = await previewProxy.set('p', 's2', first.port);
    const p2 = await previewProxy.set('p', 's2', second.port);
    // The listen port is stable across re-points: a dev-server restart on
    // a new port keeps the same preview URL (the proxy resolves the
    // current target per request).
    assert.equal(p2.listenPort, p1.listenPort, 'stable listen port across re-points');
    assert.equal(p2.devPort, second.port);

    const served = await fetchUrl(p2.url);
    assert.match(served.body, /two/);

    // Nothing is listening on this port: the proxy answers 502 with a
    // helpful message instead of hanging.
    const dead = await fakeDevServer(() => { /* never used */ });
    await dead.close();
    const deadPort = dead.port;
    const p3 = await previewProxy.set('p', 's2', deadPort);
    const unreachable = await fetchUrl(p3.url);
    assert.equal(unreachable.status, 502);
    assert.match(unreachable.body, /unreachable/);
  } finally {
    await previewProxy.clear('p', 's2');
    await first.close();
    await second.close();
  }
});

test('a preview never inherits the control plane\'s loopback owner trust', async () => {
  // The proxy connects upstream from loopback with Host rewritten to
  // localhost. Without a forwarding marker, authMiddleware would treat a
  // remote preview visitor as the local owner.
  previewProxy.configure({ basePort: TEST_BASE_PORT, maxPort: TEST_MAX_PORT, advertiseHost: '127.0.0.1', bindHost: '127.0.0.1', protectedPorts: [] });
  const app = express();
  app.use(rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
  app.get('/api/whoami', (_req, res) => { res.json({ principal: res.locals.authPrincipal }); });
  const api = http.createServer(app);
  const apiPort = await listenOutsidePreviewRange(api);
  try {
    const direct = await fetchUrl(`http://127.0.0.1:${apiPort}/api/whoami`);
    assert.equal(direct.status, 200, 'a direct loopback request is the local owner');
    const preview = await previewProxy.set('p', 'trust', apiPort);
    const proxied = await fetchUrl(`${preview.url}/api/whoami`);
    assert.notEqual(proxied.status, 200, 'a request through the preview port must not be the owner');
  } finally {
    await previewProxy.clear('p', 'trust');
    await new Promise<void>((done) => api.close(() => done()));
  }
});

test('a preview refuses the control-plane port and its own listener range', async () => {
  previewProxy.configure({ basePort: TEST_BASE_PORT, maxPort: TEST_MAX_PORT, advertiseHost: '127.0.0.1', protectedPorts: [3773] });
  await assert.rejects(previewProxy.set('p', 'cp', 3773), PreviewTargetRefusedError);
  await assert.rejects(previewProxy.set('p', 'loop', 44_910), PreviewTargetRefusedError);
  assert.equal(previewProxy.get('p', 'cp'), null);
});

test('a preview bound to one interface advertises that interface', async () => {
  // With HOST=127.0.0.1 and no advertiseHost, a Tailscale address would be
  // unreachable: the listener only accepts connections on the bind address.
  previewProxy.configure({ basePort: TEST_BASE_PORT, maxPort: TEST_MAX_PORT, advertiseHost: undefined, bindHost: '127.0.0.1', protectedPorts: [] });
  const dev = await fakeDevServer((_req, res) => { res.end('ok'); });
  try {
    const preview = await previewProxy.set('p', 'bound', dev.port);
    assert.ok(preview.url.startsWith('http://127.0.0.1:'), preview.url);
    assert.equal((await fetchUrl(preview.url)).body, 'ok');
  } finally {
    await previewProxy.clear('p', 'bound');
    await dev.close();
    previewProxy.configure({ advertiseHost: '127.0.0.1' });
  }
});
