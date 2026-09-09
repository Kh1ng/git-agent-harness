import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import rateLimit from 'express-rate-limit';
import { authMiddleware } from './authMiddleware.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import { mutationSafety } from './mutationSafety.js';
import { paidRouteApprovalsRouter } from './paidRouteApprovals.js';
import type { PaidRouteApproval } from '@git-agent-harness/contracts';

const pending: PaidRouteApproval = { profile: 'real', work_id: '#822', backend: 'opencode', backend_instance: 'paid-a', model: 'provider/model', requested: true, approved: false };
const body = { profile: pending.profile, work_id: pending.work_id, backend: pending.backend, backend_instance: pending.backend_instance, model: pending.model, confirm: true };

async function exercise(real: boolean) {
  const directory = mkdtempSync(join(tmpdir(), 'gah-paid-approval-http-'));
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const app = express();
  app.locals.deviceAccess = access;
  app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
  let rows = [{ ...pending }, { ...pending, backend_instance: 'paid-b' }];
  let changes = 0;
  const router = real ? paidRouteApprovalsRouter(mutationSafety('test-node', join(directory, 'mutations')))
    : paidRouteApprovalsRouter(mutationSafety('test-node', join(directory, 'mutations')), async () => rows,
      async (action, scope) => { changes++; rows = rows.map(row => row.backend_instance === scope.backend_instance ? { ...row, approved: action === 'grant', requested: action !== 'grant' } : row); });
  app.use('/api/route-approvals', router);
  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const endpoint = `${origin}/api/route-approvals`;
  const list = async () => { const response = await fetch(`${endpoint}?profile=real`); assert.equal(response.status, 200); return await response.json() as PaidRouteApproval[]; };
  const post = (action: string, input: object, key: string, extra: Record<string, string> = {}) => fetch(`${endpoint}/${action}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `test-${key}`,  ...extra }, body: JSON.stringify(input) });
  try {
    assert.equal((await list()).length, 2);
    const offer = access.create({ id: 'test-node', name: 'Fixture', origin });
    const paired = access.redeem(offer.code, 'test-node', origin, 'Phone');
    assert.equal((await post('grant', body, 'phone-cannot-spend-822', { Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}` })).status, 403);
    assert.equal((await post('grant', { ...body, config: '/unexpected/path' }, 'unexpected-input-822')).status, 400);
    assert.equal((await post('grant', { ...body, confirm: false }, 'unconfirmed-spend-822')).status, 400);
    assert.equal((await post('grant', { ...body, backend_instance: 'wrong-account' }, 'wrong-account-822')).status, 409);
    assert.equal((await post('grant', { ...body, model: 'other/model' }, 'wrong-model-822')).status, 409);
    assert.equal((await post('grant', { ...body, work_id: '#823' }, 'wrong-work-id-822')).status, 409);
    assert.equal((await post('grant', body, 'owner-paid-grant-822')).status, 200);
    assert.equal((await post('grant', body, 'owner-paid-grant-822')).status, 409);
    const granted = await list();
    assert.equal(granted.find(row => row.backend_instance === 'paid-a')?.approved, true);
    assert.equal(granted.find(row => row.backend_instance === 'paid-b')?.approved, false);
    if (real) {
      const configPath = process.env.GAH_CONFIG_PATH!;
      writeFileSync(configPath, readFileSync(configPath, 'utf8').replace(/\n\[profiles.real.routing.backend_instances.paid-a\][\s\S]*?(?=\n\[|$)/, ''));
    }
    assert.equal((await post('revoke', body, 'owner-paid-revoke-822')).status, 200);
    assert.equal((await list()).every(row => !row.approved), true);
    if (real) {
      const ledger = readFileSync(process.env.GAH_LEDGER_PATH!, 'utf8').trim().split('\n').map(line => JSON.parse(line));
      const grants = ledger.filter(row => row.mode === 'paid_route_approval_grant');
      const revokes = ledger.filter(row => row.mode === 'paid_route_approval_revoke');
      assert.equal(grants.length, 1); assert.equal(revokes.length, 1);
      assert.equal(grants[0].work_id, '#822');
      assert.equal(grants[0].usage.backend_instance, 'paid-a');
      assert.equal(grants[0].effective_model, 'provider/model');
    } else assert.equal(changes, 2);
    const audit = readFileSync(join(directory, 'mutations/audit.jsonl'), 'utf8');
    assert.ok(audit.includes('route_approval.grant'));
    assert.ok(audit.includes('owner_required'));
    assert.ok(audit.includes('"result":"completed"'));
    assert.ok(!audit.includes(paired.token));
  } finally { await new Promise<void>(resolve => server.close(() => resolve())); rmSync(directory, { recursive: true, force: true }); }
}

test('paid-route HTTP controls enforce exact owner decisions and durable replay protection', () => exercise(false));
test('paid-route HTTP controls change a real CLI ledger fixture', { skip: !process.env.GAH_REAL_PAID_ROUTE_TEST }, () => exercise(true));
