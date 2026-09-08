import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const run = promisify(execFile);

test('maintenance CLI sends the coordinator token and reports transport rejection without retrying', async (t) => {
  const token = 'maintenance-test-secret';
  let requests = 0;
  let rejectTransport = false;
  const server = http.createServer(async (req, res) => {
    requests += 1;
    assert.equal(req.method, 'POST');
    assert.equal(req.url, '/api/manager-chat/reclaim');
    let body = '';
    for await (const chunk of req) body += chunk;
    assert.deepEqual(JSON.parse(body), { dryRun: false });
    if (rejectTransport) {
      res.writeHead(403).end('Remote or cross-origin access requires TLS unless GAH_ALLOW_INSECURE_HTTP=1');
    } else if (req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401).end('Authentication token required');
    } else {
      res.setHeader('Content-Type', 'application/json');
      res.end(JSON.stringify({ warnings: [], candidates: [] }));
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => server.close());
  const { port } = server.address() as AddressInfo;
  const args = ['--import', 'tsx', fileURLToPath(new URL('./chatMaintenanceCli.ts', import.meta.url))];
  const options = {
    env: { ...process.env, HOST: '127.0.0.1', PORT: String(port), COORDINATOR_TOKEN: token },
    timeout: 10_000
  };
  const result = await run(process.execPath, args, options);
  assert.match(result.stdout, /Chat maintenance: 0 session\(s\)/);
  assert.equal(requests, 1);
  assert.equal(result.stderr, '');
  rejectTransport = true;
  await assert.rejects(run(process.execPath, args, options), (error: unknown) => {
    assert.ok(error instanceof Error && 'stderr' in error);
    assert.match(String(error.stderr), /403.*requires TLS.*GAH_ALLOW_INSECURE_HTTP=1/);
    assert.ok(!String(error.stderr).includes(token));
    return true;
  });
  assert.equal(requests, 2);
});
