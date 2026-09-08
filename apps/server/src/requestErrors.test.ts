import assert from 'node:assert/strict';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { test } from 'node:test';
import { withFixtureServer } from './fixtureGahHarness.js';
import { createServer } from './server.js';

test('JSON parser errors retain client status without logging or returning request contents', async (t) => {
  const logged: unknown[][] = [];
  t.mock.method(console, 'error', (...args: unknown[]) => logged.push(args));
  await withFixtureServer(async base => {
    for (const [body, status] of [
      ['{"token":"test-body-secret",', 400],
      ['"test-body-secret"', 400],
      ['null', 400],
      [JSON.stringify({ token: 'test-body-secret', padding: 'x'.repeat(110_000) }), 413]
    ] as const) {
      const response = await fetch(`${base}/api/git/pr`, {
        method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer test-header-secret' }, body
      });
      assert.equal(response.status, status);
      const text = await response.text();
      assert.ok(!text.includes('test-body-secret'));
      assert.ok(!text.includes('test-header-secret'));
    }
    const unsupported = await fetch(`${base}/api/git/pr`, {
      method: 'POST', headers: { 'Content-Type': 'application/json; charset=ascii' },
      body: '{"token":"test-body-secret"}'
    });
    assert.equal(unsupported.status, 415);
    assert.deepEqual(await unsupported.json(), { error: 'Unsupported Media Type', message: 'Unsupported request encoding' });
  });
  assert.deepEqual(logged, [], 'invalid input is not a server failure and must not log request bodies');
});

test('unexpected failures remain 500 without exposing error contents in responses or logs', async (t) => {
  const logged: unknown[][] = [];
  t.mock.method(console, 'error', (...args: unknown[]) => logged.push(args));
  const saved = process.env.GAH_ENABLE_ADMIN_UPDATE;
  process.env.GAH_ENABLE_ADMIN_UPDATE = '1';
  t.after(() => {
    if (saved === undefined) delete process.env.GAH_ENABLE_ADMIN_UPDATE;
    else process.env.GAH_ENABLE_ADMIN_UPDATE = saved;
  });
  const app = createServer({ getPendingCommits: () => { throw new Error('test-internal-secret'); } });
  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise<void>(resolve => server.close(() => resolve())));
  const { port } = server.address() as AddressInfo;
  const response = await fetch(`http://127.0.0.1:${port}/api/admin/update`);
  assert.equal(response.status, 500);
  assert.deepEqual(await response.json(), { error: 'Internal Server Error', message: 'An unexpected error occurred' });
  assert.deepEqual(logged, [['Server error: an unexpected request failure occurred']]);
});
