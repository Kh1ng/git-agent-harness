import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import express from 'express';
import { authMiddleware } from './authMiddleware.js';
import {
  runTelemetryAggregate,
  runClaimsList,
  runQuotaList,
  runExternalApprovalInspect,
} from './gahCli.js';
import type { ExternalApprovalScope } from '@git-agent-harness/contracts';

// Issue #519: the four JSON-ready read adapters must expose typed responses,
// validate query inputs, and never leak local filesystem layout (the
// external-approval projection drops the CLI's ledger_path).

const noopAuth: express.RequestHandler = (_req, _res, next) => next();

/* eslint-disable @typescript-eslint/no-explicit-any */
type AnyStub = (...args: any[]) => Promise<unknown>;

function appWith(stubs: Record<string, AnyStub>) {
  const app = express();
  app.use(express.json(), noopAuth);
  app.get('/api/telemetry/aggregate', async (req, res) => {
    try {
      const dimensions = typeof req.query.dimensions === 'string' ? req.query.dimensions.split(',') : [];
      if (dimensions.length === 0) return res.status(400).json({ error: 'dimensions_required' });
      return res.json(await (stubs.telemetry as AnyStub)({ dimensions }));
    } catch (error) {
      return res.status(502).json({ error: 'telemetry_aggregate_unavailable' });
    }
  });
  app.get('/api/claims', async (req, res) => {
    try {
      const profile = typeof req.query.profile === 'string' ? req.query.profile : undefined;
      return res.json(await (stubs.claims as AnyStub)(profile));
    } catch {
      return res.status(502).json({ error: 'claims_unavailable' });
    }
  });
  app.get('/api/quota/list', async (_req, res) => {
    try {
      return res.json(await (stubs.quota as AnyStub)());
    } catch {
      return res.status(502).json({ error: 'quota_list_unavailable' });
    }
  });
  app.get('/api/external-approval/inspect', async (req, res) => {
    const profile = req.query.profile as string | undefined;
    const workId = req.query.work_id as string | undefined;
    const credentialLabel = req.query.credential_label as string | undefined;
    const operationKind = req.query.operation_kind as string | undefined;
    if (!profile || !workId || !credentialLabel || !operationKind) {
      return res.status(400).json({ error: 'scope_required' });
    }
    try {
      return res.json(await (stubs.inspect as AnyStub)({ profile, workId, credentialLabel, operationKind }));
    } catch {
      return res.status(502).json({ error: 'external_approval_unavailable' });
    }
  });
  return app;
}

const openServers: http.Server[] = [];

async function listen(app: express.Express): Promise<string> {
  const server = http.createServer(app);
  openServers.push(server);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return origin;
}

test('read adapters expose typed responses, validate inputs, and drop local paths', async () => {
  const scope: ExternalApprovalScope = {
    profile: 'real',
    repo_id: 'owner/real',
    work_id: '#653',
    credential_label: 'external-api-key',
    operation_kind: 'api_call',
    state: 'granted',
    active: true,
    allowed_env_vars: ['EXTERNAL_API_KEY'],
    max_requests: 50,
    max_dollars: 5,
    expires_at: '2026-12-01T00:00:00Z',
    purpose: 'pay per call',
    consumed_requests: 3,
    consumed_dollars: 0.4,
    denial_reason: null,
  };
  let inspectArgs: { profile: string; workId: string; credentialLabel: string; operationKind: string } | null = null;
  const app = appWith({
    telemetry: async params => {
      assert.deepEqual(params.dimensions, ['project', 'model']);
      return {
        report_type: 'aggregation',
        generated_at: '2026-09-10T00:00:00Z',
        time_range: null,
        profile: null,
        total_entries: 1,
        total_attempts: 1,
        successful_attempts: 1,
        failed_attempts: 0,
        total_cost_usd: 0.1,
        quota_backed_cost_usd: 0,
        api_cost_usd: 0.1,
        aggregated_data: [],
      };
    },
    claims: async profile => {
      assert.equal(profile, 'real');
      return [{ work_id: '#1', pid: 123, hostname: 'worker-a', claimed_at: '2026-09-10T00:00:00Z', is_stale: false }];
    },
    quota: async () => [{ backend: 'codex', quota_used_percent: 10 }],
    inspect: async params => {
      inspectArgs = params;
      return scope;
    },
  });
  const origin = await listen(app);

  const aggregate = (await (await fetch(`${origin}/api/telemetry/aggregate?dimensions=project,model`)).json()) as import('@git-agent-harness/contracts').TelemetryAggregateReport;
  assert.equal(aggregate.report_type, 'aggregation');
  assert.equal((await fetch(`${origin}/api/telemetry/aggregate`)).status, 400);

  const claims = (await (await fetch(`${origin}/api/claims?profile=real`)).json()) as import('@git-agent-harness/contracts').WorkClaimDetail[];
  assert.equal(claims.length, 1);
  assert.equal(claims[0].work_id, '#1');

  const quota = (await (await fetch(`${origin}/api/quota/list`)).json()) as import('@git-agent-harness/contracts').QuotaListRecord[];
  assert.equal(quota[0].backend, 'codex');

  const inspected = (await (await fetch(`${origin}/api/external-approval/inspect?profile=real&work_id=%23653&credential_label=external-api-key&operation_kind=api_call`)).json()) as ExternalApprovalScope;
  assert.equal(inspected.state, 'granted');
  assert.equal('ledger_path' in inspected, false, 'local ledger path must never reach remote callers');
  assert.deepEqual(inspectArgs, { profile: 'real', workId: '#653', credentialLabel: 'external-api-key', operationKind: 'api_call' });

  assert.equal((await fetch(`${origin}/api/external-approval/inspect?profile=real&work_id=%23653`)).status, 400);
  for (const server of openServers.splice(0)) {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
  }
});

test('read adapters surface CLI failures as typed 502s', async () => {
  const app = appWith({
    telemetry: async () => { throw new Error('gah telemetry aggregate failed'); },
    claims: async () => { throw new Error('gah claims list failed'); },
    quota: async () => { throw new Error('gah quota list failed'); },
    inspect: async () => { throw new Error('gah external-approval inspect failed'); },
  });
  const origin = await listen(app);
  for (const path of ['/api/telemetry/aggregate?dimensions=model', '/api/claims', '/api/quota/list', '/api/external-approval/inspect?profile=p&work_id=w&credential_label=c&operation_kind=o']) {
    const response = await fetch(`${origin}${path}`);
    assert.equal(response.status, 502, path);
  }
  for (const server of openServers.splice(0)) {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
  }
});
