import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { mutationSafety } from './mutationSafety.js';
import { withFixtureServer } from './fixtureGahHarness.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';

test('mutation receipts prevent concurrent repeats and survive restart without storing secrets or response bodies', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-mutations-'));
  let calls = 0;
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  const servers: http.Server[] = [];
  const access = new DeviceAccess(join(directory, 'devices.json'));
  async function listen(node = 'test-node') {
    const app = express();
    app.locals.deviceAccess = access;
    app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
    const mutation = mutationSafety(node, directory);
    app.post('/hold', mutation('hold.set'), async (_req, res) => {
      calls++; entered(); await pending;
      res.json({ ok: true, message: 'secret-from-command-output' });
    });
    app.post('/clear', mutation('ledger.clear_attempts'), (_req, res) => { calls++; res.json({ ok: true }); });
    const server = http.createServer(app);
    servers.push(server);
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
    return `http://127.0.0.1:${(server.address() as AddressInfo).port}/hold`;
  }
  const key = 'unique-operation-0001';
  const post = (url: string, body: object, id = key) => fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': id }, body: JSON.stringify(body) });
  try {
    const url = await listen();
    assert.equal((await post(url, {}, '')).status, 400);
    const first = post(url, { profile: 'fixture', workId: '42', reason: 'secret-free-text-reason' });
    await started;
    const duplicate = await post(url, { reason: 'secret-free-text-reason', workId: '42', profile: 'fixture' });
    assert.equal(duplicate.status, 409);
    assert.equal((await duplicate.json() as { error: string }).error, 'mutation_already_accepted');
    const conflict = await post(url, { profile: 'different' });
    assert.equal(conflict.status, 409);
    assert.equal((await conflict.json() as { error: string }).error, 'idempotency_conflict');
    assert.equal(calls, 1);
    release();
    assert.equal((await first).status, 200);
    const restarted = await listen();
    assert.equal((await post(restarted, { profile: 'fixture', workId: '42', reason: 'secret-free-text-reason' })).status, 409);
    assert.equal(calls, 1);
    assert.equal((await post(await listen('different-node'), { profile: 'fixture', workId: '42', reason: 'secret-free-text-reason' })).status, 200);
    assert.equal(calls, 2, 'Node identity namespaces receipts');
    const origin = new URL(url).origin;
    const offer = access.create({ id: 'test-node', name: 'Fixture', origin });
    const paired = access.redeem(offer.code, 'test-node', origin, 'Phone');
    const deviceRequest = (path: string) => fetch(origin + path, { method: 'POST', headers: {
      'Content-Type': 'application/json', 'Idempotency-Key': key, Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}`,
    }, body: JSON.stringify({ profile: 'fixture', workId: '42', reason: 'secret-free-text-reason' }) });
    assert.equal((await deviceRequest('/hold')).status, 200, 'Different authenticated actors have separate receipts');
    assert.equal(calls, 3);
    assert.equal((await deviceRequest('/clear')).status, 403, 'Paired devices cannot clear history');
    assert.equal(calls, 3);
    for (const name of readdirSync(directory)) {
      const path = join(directory, name);
      const contents = readFileSync(path, 'utf8');
      assert.ok(!contents.includes('secret-'));
      assert.ok(!contents.includes(key));
      assert.equal(statSync(path).mode & 0o777, 0o600);
    }
    const records = readFileSync(join(directory, 'audit.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.equal(records.filter(record => record.result === 'accepted').length, 3);
    assert.equal(records.filter(record => record.result === 'completed').length, 3);
    assert.ok(records.every(record => ['owner', `device:${paired.device.id}`].includes(record.actor) && /^[a-f0-9]{64}$/.test(record.target_digest)));
  } finally {
    release();
    await Promise.all(servers.map(server => new Promise<void>(resolve => server.close(() => resolve()))));
    rmSync(directory, { recursive: true, force: true });
  }
});

test('unavailable audit storage prevents execution and keeps a durable reservation', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-mutations-failure-'));
  const app = express();
  app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
  let calls = 0;
  app.post('/stop', mutationSafety('node', directory)('loop.stop'), (_req, res) => {
    calls++;
    // Simulate terminal persistence failure after the action has executed.
    rmSync(join(directory, 'audit.jsonl'));
    mkdirSync(join(directory, 'audit.jsonl'));
    res.json({ stopped: true });
  });
  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/stop`;
  const post = (key = 'persistent-request-1') => fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': key }, body: '{}' });
  try {
    // A directory in place of the audit file makes appending fail on all hosts.
    mkdirSync(join(directory, 'audit.jsonl'));
    assert.equal((await post()).status, 503);
    assert.equal(calls, 0);
    rmSync(join(directory, 'audit.jsonl'), { recursive: true });
    assert.equal((await post()).status, 409, 'An unacknowledged reservation remains reserved');
    assert.equal(calls, 0);
    const receipt = readdirSync(directory).find(name => name.endsWith('.json'))!;
    writeFileSync(join(directory, receipt), '{');
    assert.equal((await post()).status, 503, 'Partial receipts fail closed');
    assert.equal(calls, 0);
    const terminal = await post('terminal-failure-0001');
    assert.equal(terminal.status, 503);
    assert.equal((await terminal.json() as { error: string }).error, 'mutation_outcome_unknown');
    assert.equal(calls, 1);
    rmSync(join(directory, 'audit.jsonl'), { recursive: true });
    assert.equal((await post('terminal-failure-0001')).status, 409);
    assert.equal(calls, 1, 'Failed completion cannot rerun the operation');
    writeFileSync(join(directory, 'audit.jsonl'), '{"partial":');
    assert.equal((await post('torn-audit-line-0001')).status, 503);
    assert.equal(calls, 1, 'A torn audit line blocks new actions');
  } finally {
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test('all six production mutation routes require keys before invoking CLI actions', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-mutations-routes-'));
  const saved = process.env.GAH_MUTATION_STORE_PATH;
  process.env.GAH_MUTATION_STORE_PATH = directory;
  try {
    await withFixtureServer(async base => {
      for (const route of ['loop/start', 'loop/stop', 'hold/set', 'hold/clear', 'availability/clear', 'ledger/clear-attempts']) {
        const res = await fetch(`${base}/api/${route}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
        assert.equal(res.status, 400, route);
        assert.equal((await res.json() as { error: string }).error, 'idempotency_key_required', route);
      }
    });
  } finally {
    if (saved === undefined) delete process.env.GAH_MUTATION_STORE_PATH; else process.env.GAH_MUTATION_STORE_PATH = saved;
    rmSync(directory, { recursive: true, force: true });
  }
});
