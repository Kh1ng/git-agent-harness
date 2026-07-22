import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtempSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { test } from 'node:test';

import { WebSocketServer } from 'ws';
import type { RegistryService } from './registryService.js';
import type { NodeObservationSnapshot, ServerMessage, Session } from '@git-agent-harness/contracts';

import { createFleetDispatchCoordinator } from './fleetDispatch.js';

type PublishedMessage = ServerMessage;

type MockNodeHandle = {
  url: string;
  sessions: Session[];
  close: () => Promise<void>;
};

function makeSnapshot(
  nodeId: string,
  advertisedUrl: string,
  cpuPercent: number,
  activeWorkCount: number,
  activeClaimCount: number
): NodeObservationSnapshot {
  return {
    node_id: nodeId,
    display_name: nodeId,
    advertised_url: advertisedUrl,
    version: '0.1.0',
    schema_digest: 'schema',
    state: 'healthy',
    observed_at: new Date().toISOString(),
    last_seen_at: new Date().toISOString(),
    last_observed_state: 'healthy',
    last_error_kind: null,
    last_error_message: null,
    profile: 'gah',
    profiles: ['gah'],
    backend_configured: { codex: true },
    backend_instances: [],
    availability: [],
    recent_ledger: null,
    active_claims: Array.from({ length: activeClaimCount }, (_, index) => ({
      work_id: `${nodeId}-work-${index}`,
      pid: 10_000 + index,
      scope: 'repo',
      hostname: 'localhost',
      claimed_at: new Date().toISOString(),
      age_seconds: index
    })),
    active_work: Array.from({ length: activeWorkCount }, (_, index) => ({
      node_id: nodeId,
      work_id: `${nodeId}-work-${index}`,
      node_qualified_work_id: `${nodeId}-qualified-work-${index}`,
      scope: 'repo',
      hostname: 'localhost',
      claimed_at: new Date().toISOString(),
      age_seconds: index
    })),
    event_cursor: null,
    resource_pressure: {
      cpu_percent: cpuPercent,
      rss_bytes: 128_000_000,
      disk_percent: cpuPercent
    },
    error: null
  };
}

async function createMockNode(initialSessions: Session[] = []): Promise<MockNodeHandle> {
  let sessions = [...initialSessions];
  const server = http.createServer();
  const wss = new WebSocketServer({ server });

  wss.on('connection', (socket) => {
    socket.on('message', (raw) => {
      try {
        const message = JSON.parse(raw.toString()) as { type?: string };
        if (message.type === 'client.hello') {
          socket.send(
            JSON.stringify({
              type: 'server.welcome',
              serverVersion: '0.1.0',
              serverProviderCatalog: { providers: [] },
              sessions,
              providers: {},
              profile: 'gah'
            } satisfies ServerMessage)
          );
        }
      } catch {
        // Ignore malformed probe messages.
      }
    });
  });

  await new Promise<void>((resolveListen) => {
    server.listen(0, '127.0.0.1', resolveListen);
  });

  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Mock node server did not bind to a TCP port');
  }

  return {
    url: `http://127.0.0.1:${address.port}`,
    get sessions() {
      return sessions;
    },
    set sessions(next: Session[]) {
      sessions = [...next];
    },
    close: async () => {
      await new Promise<void>((resolveClose) => {
        wss.close(() => resolveClose());
      });
      await new Promise<void>((resolveClose) => {
        server.close(() => resolveClose());
      });
    }
  };
}

class FakeTransport {
  private sessions = new Map<string, Session>();
  public startCalls = 0;
  public stopCalls = 0;

  constructor(
    private readonly nodeId: string,
    private readonly onTerminal: (session: Session) => void
  ) {}

  async startSession(options: {
    requestId?: string;
    providerKind: Session['providerKind'];
    instanceId: Session['instanceId'];
    repo: string;
    branch?: string;
    target?: string;
    mode: string;
    backend?: string;
    model?: string;
    budget?: number;
  }): Promise<Session> {
    this.startCalls += 1;
    const session: Session = {
      id: options.requestId ?? `${this.nodeId}-${this.startCalls}`,
      providerKind: options.providerKind,
      instanceId: options.instanceId,
      status: 'running',
      startedAt: new Date().toISOString(),
      repo: options.repo,
      branch: options.branch,
      target: options.target,
      mode: options.mode,
      backend: options.backend,
      model: options.model,
      budget: options.budget
    };
    this.sessions.set(session.id, session);
    return session;
  }

  async stopSession(sessionId: string): Promise<Session> {
    this.stopCalls += 1;
    const session = this.sessions.get(sessionId);
    if (!session) {
      throw new Error(`Missing session ${sessionId}`);
    }
    const stopped: Session = {
      ...session,
      status: 'stopped',
      endedAt: new Date().toISOString()
    };
    this.sessions.set(sessionId, stopped);
    this.onTerminal(stopped);
    return stopped;
  }

  async sendCommand(): Promise<void> {}

  getSession(sessionId: string): Session | undefined {
    return this.sessions.get(sessionId);
  }

  getSessions(): Session[] {
    return Array.from(this.sessions.values());
  }

  async close(): Promise<void> {}
}

function createCoordinatorHarness(options: {
  leaseStorePath: string;
  registryService: RegistryService;
  coordinatorNodeId: string;
  coordinatorUrl: string;
  transportMap: Map<string, FakeTransport>;
  published: PublishedMessage[];
}) {
  const { leaseStorePath, registryService, coordinatorNodeId, coordinatorUrl, transportMap, published } = options;
  return createFleetDispatchCoordinator({
    registryService,
    pushBus: {
      publish(message) {
        published.push(message);
      }
    },
    coordinatorIdentity: {
      node_id: coordinatorNodeId,
      display_name: 'GAH Coordinator',
      advertised_url: coordinatorUrl,
      version: '0.1.0',
      schema_digest: 'schema'
    },
    leaseStorePath,
    transportFactory: (node, context) => {
      let transport = transportMap.get(node.nodeId);
      if (!transport) {
        transport = new FakeTransport(node.nodeId, context.onTerminal);
        transportMap.set(node.nodeId, transport);
      }
      return transport as any;
    }
  });
}

test('fleet dispatch routes to the least-loaded healthy node and honors explicit pins', async () => {
  const mockNode = await createMockNode();
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('coordinator-1', 'http://127.0.0.1:9999', 90, 4, 2),
        makeSnapshot('worker-1', mockNode.url, 10, 0, 0)
      ];
    }
  } as unknown as RegistryService;

  const coordinator = createCoordinatorHarness({
    leaseStorePath,
    registryService,
    coordinatorNodeId: 'coordinator-1',
    coordinatorUrl: 'http://127.0.0.1:9999',
    transportMap,
    published
  });

  try {
    const routed = await coordinator.startSession({
      requestId: 'dispatch-load-aware',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-4.1'
    });

    assert.equal(routed.nodeId, 'worker-1');
    assert.equal(routed.leaseState, 'running');
    assert.equal(transportMap.get('worker-1')?.startCalls, 1);
    assert.equal(transportMap.get('coordinator-1')?.startCalls ?? 0, 0);
    assert.equal(
      published.filter((message) => message.type === 'session.started').length,
      1
    );

    const pinned = await coordinator.startSession({
      requestId: 'dispatch-pinned',
      nodeId: 'coordinator-1',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve'
    });

    assert.equal(pinned.nodeId, 'coordinator-1');
    assert.equal(transportMap.get('coordinator-1')?.startCalls, 1);

    await assert.rejects(
      coordinator.startSession({
        requestId: 'dispatch-bad-pin',
        nodeId: 'missing-node',
        profile: 'gah',
        providerKind: 'codex',
        instanceId: 'codex-0',
        repo: 'owner/repo',
        mode: 'improve'
      }),
      (error: unknown) =>
        error instanceof Error &&
        'code' in error &&
        (error as { code?: string }).code === 'NODE_PINNING_FAILED'
    );
  } finally {
    await mockNode.close();
  }
});

test('fleet dispatch reuses request ids across coordinator restarts and reconciles node state', async () => {
  const mockNode = await createMockNode();
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();
  const coordinatorIdentity = {
    node_id: 'coordinator-1',
    display_name: 'GAH Coordinator',
    advertised_url: 'http://127.0.0.1:9999',
    version: '0.1.0',
    schema_digest: 'schema'
  };

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('coordinator-1', coordinatorIdentity.advertised_url, 90, 4, 2),
        makeSnapshot('worker-1', mockNode.url, 10, 0, 0)
      ];
    }
  } as unknown as RegistryService;

  const firstCoordinator = createCoordinatorHarness({
    leaseStorePath,
    registryService,
    coordinatorNodeId: coordinatorIdentity.node_id,
    coordinatorUrl: coordinatorIdentity.advertised_url,
    transportMap,
    published
  });

  try {
    const first = await firstCoordinator.startSession({
      requestId: 'dispatch-restart',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve'
    });

    mockNode.sessions = [first];

    const secondTransportMap = new Map<string, FakeTransport>();
    const restartedCoordinator = createCoordinatorHarness({
      leaseStorePath,
      registryService,
      coordinatorNodeId: coordinatorIdentity.node_id,
      coordinatorUrl: coordinatorIdentity.advertised_url,
      transportMap: secondTransportMap,
      published
    });

    const retry = await restartedCoordinator.startSession({
      requestId: 'dispatch-restart',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve'
    });

    assert.equal(retry.id, first.id);
    assert.equal(secondTransportMap.size, 0);
    assert.equal(
      published.filter((message) => message.type === 'session.started').length,
      1
    );

    await restartedCoordinator.reconcileLeases('gah');

    const reconciled = restartedCoordinator.getSession(first.id);
    assert.equal(reconciled?.leaseState, 'running');
    assert.equal(reconciled?.nodeId, 'worker-1');
  } finally {
    await mockNode.close();
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});
