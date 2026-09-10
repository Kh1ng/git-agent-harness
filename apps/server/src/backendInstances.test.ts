import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import { mutationSafety } from './mutationSafety.js';
import { backendInstancesRouter } from './backendInstances.js';
import type { BackendInstanceSummary, ConfigProfileSummary } from '@git-agent-harness/contracts';

const instance = (name: string, enabled: boolean): BackendInstanceSummary => ({
  backend_instance: name,
  runner_kind: 'codex',
  enabled,
  logical_backend: 'codex',
  account_label: null,
  auth_source_label: null,
  quota_pool: null,
  supported_models: ['openai/gpt-5'],
  executable_configured: true,
  isolated_state_configured: true,
});

async function exercise(real: boolean) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-backend-instance-http-'));
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const app = express();
  app.locals.deviceAccess = access;
  app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
  let rows = [instance('codex-paid', true), instance('codex-broken', false)];
  let toggles: Array<{ profile: string; instance: string; enabled: boolean }> = [];
  const router = real
    ? backendInstancesRouter(mutationSafety('test-node', join(directory, 'mutations')))
    : backendInstancesRouter(
        mutationSafety('test-node', join(directory, 'mutations')),
        async (profile: string) => {
          assert.equal(profile, 'real');
          return { backend_instances: rows } as unknown as ConfigProfileSummary;
        },
        async (profile: string, instanceName: string, enabled: boolean) => {
          toggles.push({ profile, instance: instanceName, enabled });
          rows = rows.map(row => (row.backend_instance === instanceName ? { ...row, enabled } : row));
        }
      );
  app.use('/api/backend-instances', router);
  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const endpoint = `${origin}/api/backend-instances`;
  const list = async () => {
    const response = await fetch(`${endpoint}?profile=real`);
    assert.equal(response.status, 200);
    return (await response.json()) as { profile: string; backend_instances: BackendInstanceSummary[] };
  };
  const post = (action: string, input: object, key: string, extra: Record<string, string> = {}) =>
    fetch(`${endpoint}/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `test-${key}`, ...extra },
      body: JSON.stringify(input),
    });
  try {
    assert.equal((await list()).backend_instances.length, 2);
    const offer = access.create({ id: 'test-node', name: 'Fixture', origin });
    const paired = access.redeem(offer.code, 'test-node', origin, 'Phone');
    // Paired (non-owner) devices may read but never toggle.
    assert.equal((await post('disable', { profile: 'real', instance: 'codex-paid' }, 'phone-toggle', { Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}` })).status, 403);
    // Unexpected fields rejected.
    assert.equal((await post('disable', { profile: 'real', instance: 'codex-paid', config: '/unexpected/path' }, 'unexpected-input')).status, 400);
    assert.equal((await post('disable', { profile: 'real', instance: '' }, 'empty-instance')).status, 400);
    // Unknown instance is a conflict, not a blind write.
    assert.equal((await post('disable', { profile: 'real', instance: 'ghost' }, 'ghost-instance')).status, 409);
    // Owner toggle disable then enable.
    assert.equal((await post('disable', { profile: 'real', instance: 'codex-paid' }, 'owner-disable')).status, 200);
    assert.equal((await list()).backend_instances.find(row => row.backend_instance === 'codex-paid')?.enabled, false);
    if (!real) assert.equal(toggles.filter(entry => entry.instance === 'codex-paid' && entry.enabled === false).length, 1);
    // Idempotent repeat: no second CLI invocation, still 200.
    assert.equal((await post('disable', { profile: 'real', instance: 'codex-paid' }, 'owner-disable-again')).status, 200);
    if (!real) assert.equal(toggles.filter(entry => entry.instance === 'codex-paid' && entry.enabled === false).length, 1);
    assert.equal((await post('enable', { profile: 'real', instance: 'codex-paid' }, 'owner-enable')).status, 200);
    assert.equal((await list()).backend_instances.find(row => row.backend_instance === 'codex-paid')?.enabled, true);
    const audit = (await import('node:fs')).readFileSync(join(directory, 'mutations/audit.jsonl'), 'utf8');
    assert.ok(audit.includes('backend_instance.set_enabled'));
    assert.ok(audit.includes('owner_required'));
  } finally {
    server.close();
    rmSync(directory, { recursive: true, force: true });
  }
}

test('backend instance toggles are owner-gated, validated, and audit-logged', async () => {
  await exercise(false);
});
