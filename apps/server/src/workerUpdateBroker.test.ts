import assert from 'node:assert/strict';
import { test } from 'node:test';
import { WorkerUpdateBroker, WorkerUpdateError } from './workerUpdateBroker.js';
import type { RegisteredNode, WorkerUpdateStatus } from '@git-agent-harness/contracts';

const idleStatus: WorkerUpdateStatus = {
  status: 'idle',
  current_version: '0.1.2',
  target_version: null,
  armed_at: null,
  started_at: null,
  finished_at: null,
  active_dispatches: null,
  output: ''
};

function node(overrides: Partial<RegisteredNode> = {}): RegisteredNode {
  return {
    node_id: 'worker-1',
    display_name: 'Worker One',
    advertised_url: 'http://127.0.0.1:3774',
    version: '0.1.2',
    schema_digest: 'digest',
    transport_mode: 'trusted_lan',
    secret_ref: 'env:GAH_NODE_TOKEN',
    ...overrides
  };
}

interface RegistryStub {
  getNode: (nodeId: string) => RegisteredNode | undefined;
  getNodes: () => RegisteredNode[];
  getNodesSummary: () => import('@git-agent-harness/contracts').NodeSummary[];
}

function registryStub(nodes: RegisteredNode[], summaries?: import('@git-agent-harness/contracts').NodeSummary[]): RegistryStub {
  return {
    getNode: (nodeId) => nodes.find((candidate) => candidate.node_id === nodeId),
    getNodes: () => nodes,
    getNodesSummary: () => summaries ?? nodes.map(({ secret_ref: _secret, ...summary }) => ({
      ...summary,
      update: {
        status: 'behind' as const,
        node_version: summary.version,
        coordinator_version: '0.1.3',
        minimum_worker_version: '0.1.3'
      }
    }))
  };
}

function fetchStub(handler: (url: string, body: Record<string, unknown>) => { status: number; body: unknown }): typeof fetch {
  return (async (input: string | URL, init?: RequestInit) => {
    const url = String(input);
    const body = JSON.parse(String(init?.body ?? '{}')) as Record<string, unknown>;
    const answer = handler(url, body);
    return new Response(JSON.stringify(answer.body), {
      status: answer.status,
      headers: { 'content-type': 'application/json' }
    });
  }) as unknown as typeof fetch;
}

test('start posts to the worker and surfaces its status', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node()]),
    localNodeId: 'central-1',
    fetch: fetchStub((url, body) => {
      assert.ok(url.endsWith('/api/worker-update'), url);
      assert.equal(body.action, 'start');
      return { status: 200, body: { started: true, status: { ...idleStatus, status: 'waiting', active_dispatches: 1 } } };
    })
  });
  const result = await broker.start('worker-1');
  assert.equal(result.started, true);
  assert.equal(result.status.status, 'waiting');
  assert.equal(result.status.active_dispatches, 1);
});

test('start rejects an unknown node and the coordinator itself', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node()]),
    localNodeId: 'central-1',
    fetch: fetchStub(() => ({ status: 200, body: {} }))
  });
  await assert.rejects(broker.start('nope'), (error: unknown) => {
    assert.ok(error instanceof WorkerUpdateError);
    assert.equal(error.status, 404);
    return true;
  });
  await assert.rejects(broker.start('central-1'), (error: unknown) => {
    assert.ok(error instanceof WorkerUpdateError);
    assert.equal(error.status, 400);
    return true;
  });
});

test('non-loopback plain HTTP is refused without the insecure opt-in', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node({ advertised_url: 'http://100.64.0.5:3774', transport_mode: 'trusted_lan' })]),
    localNodeId: 'central-1',
    fetch: fetchStub(() => ({ status: 200, body: {} }))
  });
  const saved = process.env.GAH_ALLOW_INSECURE_HTTP;
  delete process.env.GAH_ALLOW_INSECURE_HTTP;
  try {
    await assert.rejects(broker.start('worker-1'), (error: unknown) => {
      assert.ok(error instanceof WorkerUpdateError);
      assert.equal(error.status, 502);
      assert.match(error.message, /HTTPS/);
      return true;
    });
  } finally {
    if (saved === undefined) delete process.env.GAH_ALLOW_INSECURE_HTTP;
    else process.env.GAH_ALLOW_INSECURE_HTTP = saved;
  }
});

test('worker errors map to fixed statuses and messages, never raw output', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node()]),
    localNodeId: 'central-1',
    fetch: fetchStub(() => ({ status: 409, body: { message: 'Update already running.' } }))
  });
  await assert.rejects(broker.start('worker-1'), (error: unknown) => {
    assert.ok(error instanceof WorkerUpdateError);
    assert.equal(error.status, 409);
    assert.equal(error.message, 'Update already running.');
    return true;
  });
});

test('a malformed worker response is a protocol error, not a fake success', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node()]),
    localNodeId: 'central-1',
    fetch: fetchStub(() => ({ status: 200, body: { started: 'yes' } }))
  });
  await assert.rejects(broker.start('worker-1'), (error: unknown) => {
    assert.ok(error instanceof WorkerUpdateError);
    assert.equal(error.status, 502);
    return true;
  });
});

test('updateAll reports per-node outcomes and never aborts the sweep on one failure', async () => {
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node(), node({ node_id: 'worker-2', display_name: 'Worker Two', advertised_url: 'http://127.0.0.1:3775' })]),
    localNodeId: 'central-1',
    fetch: fetchStub((_url, body) => {
      if (body.action === 'status') return { status: 200, body: { started: true, status: idleStatus } };
      return { status: 200, body: { started: true, status: { ...idleStatus, status: 'waiting' } } };
    })
  });
  const results = await broker.updateAll();
  assert.equal(results.length, 2);
  assert.equal(results[0].started, true);
  assert.equal(results[0].status?.status, 'waiting');
  assert.equal(results[1].error, null);
});

test('autoUpdateSweep only touches opted-in nodes the registry sees behind', async () => {
  const summaries: import('@git-agent-harness/contracts').NodeSummary[] = [
    {
      node_id: 'central-1',
      display_name: 'Central',
      advertised_url: 'http://127.0.0.1:3773',
      version: '0.1.3',
      schema_digest: 'digest',
      transport_mode: 'loopback',
      auto_update: true,
      update: { status: 'current', node_version: '0.1.3', coordinator_version: '0.1.3', minimum_worker_version: '0.1.3' }
    },
    {
      node_id: 'worker-1',
      display_name: 'Worker One',
      advertised_url: 'http://127.0.0.1:3774',
      version: '0.1.2',
      schema_digest: 'digest',
      transport_mode: 'loopback',
      auto_update: false,
      update: { status: 'behind', node_version: '0.1.2', coordinator_version: '0.1.3', minimum_worker_version: '0.1.3' }
    },
    {
      node_id: 'worker-2',
      display_name: 'Worker Two',
      advertised_url: 'http://127.0.0.1:3775',
      version: '0.1.2',
      schema_digest: 'digest',
      transport_mode: 'loopback',
      auto_update: true,
      update: { status: 'behind', node_version: '0.1.2', coordinator_version: '0.1.3', minimum_worker_version: '0.1.3' }
    }
  ];
  let called: string[] = [];
  const broker = new WorkerUpdateBroker({
    registry: registryStub([node(), node({ node_id: 'worker-2', advertised_url: 'http://127.0.0.1:3775' })], summaries),
    localNodeId: 'central-1',
    fetch: fetchStub((_url, body) => {
      called.push(String(body.action));
      return { status: 200, body: { started: true, status: idleStatus } };
    })
  });
  const results = await broker.autoUpdateSweep();
  assert.equal(results.length, 1, 'only the opted-in behind worker is swept');
  assert.equal(results[0].node_id, 'worker-2');
  assert.deepEqual(called, ['start']);
});
