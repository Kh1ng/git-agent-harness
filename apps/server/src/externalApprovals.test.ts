import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import rateLimit from 'express-rate-limit';
import type { ExternalApprovalDecisionScope, ExternalApprovalScope } from '@git-agent-harness/contracts';
import { authMiddleware } from './authMiddleware.js';
import { DeviceAccess, DEVICE_COOKIE } from './deviceAccess.js';
import { externalApprovalsRouter } from './externalApprovals.js';
import { mutationSafety } from './mutationSafety.js';

const pending = (label: string): ExternalApprovalScope => ({ profile: 'real', repo_id: 'owner/repo', work_id: '#653', credential_label: label, operation_kind: 'env_credential', state: 'requested', active: false, allowed_env_vars: [`${label.toUpperCase()}_API_KEY`], max_requests: 1, max_dollars: 2.5, expires_at: null, purpose: 'test', consumed_requests: 0, consumed_dollars: null, denial_reason: null });

test('external approval HTTP controls enforce exact owner decisions without widening scope', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-external-approval-http-'));
  const access = new DeviceAccess(join(directory, 'devices.json'));
  const app = express();
  app.locals.deviceAccess = access;
  app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware);
  let rows = [pending('odds'), pending('maps')];
  const changes: Array<{ action: string; scope: ExternalApprovalDecisionScope }> = [];
  app.use('/api/external-approvals', externalApprovalsRouter(
    mutationSafety('test-node', join(directory, 'mutations')),
    async () => rows,
    async (action, scope) => {
      changes.push({ action, scope });
      rows = rows.map(row => row.credential_label === scope.credential_label
        ? { ...row, state: action === 'grant' ? 'approved' : action === 'deny' ? 'denied' : 'revoked', active: action === 'grant', denial_reason: action === 'deny' ? 'denied by operator' : null }
        : row);
    },
  ));
  const server = http.createServer(app);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const endpoint = `${origin}/api/external-approvals`;
  const scope: ExternalApprovalDecisionScope = { profile: 'real', work_id: '#653', credential_label: 'odds', operation_kind: 'env_credential' };
  const post = (action: string, body: object, key: string, extra: Record<string, string> = {}) => fetch(`${endpoint}/${action}`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `test-${key}`, ...extra }, body: JSON.stringify(body) });
  try {
    assert.equal((await fetch(`${endpoint}?profile=real`)).status, 200);
    const offer = access.create({ id: 'test-node', name: 'Fixture', origin });
    const paired = access.redeem(offer.code, 'test-node', origin, 'Phone');
    assert.equal((await post('grant', { ...scope, confirm: true }, 'phone-cannot-grant-653', { Origin: origin, Cookie: `${DEVICE_COOKIE}=${paired.token}` })).status, 403);
    assert.equal((await post('grant', { ...scope, max_requests: 10, confirm: true }, 'cannot-widen-653')).status, 400);
    assert.equal((await post('grant', { ...scope, credential_label: 'other', confirm: true }, 'wrong-scope-653')).status, 409);
    assert.equal((await post('grant', { ...scope, confirm: true }, 'owner-grant-653')).status, 200);
    assert.deepEqual(changes[0], { action: 'grant', scope });
    assert.deepEqual(Object.keys(changes[0].scope).sort(), ['credential_label', 'operation_kind', 'profile', 'work_id']);
    assert.equal((await post('revoke', { ...scope, confirm: true }, 'owner-revoke-653')).status, 200);
    assert.equal((await post('deny', { ...scope, credential_label: 'maps', confirm: true }, 'owner-deny-653')).status, 200);
    const audit = readFileSync(join(directory, 'mutations/audit.jsonl'), 'utf8');
    assert.ok(audit.includes('external_approval.grant'));
    assert.ok(audit.includes('owner_required'));
    assert.ok(audit.includes('external_approval.deny'));
    assert.ok(!audit.includes(paired.token));
  } finally {
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  }
});
