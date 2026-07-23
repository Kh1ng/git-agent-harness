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
  activeClaimCount: number,
  overrides: Partial<NodeObservationSnapshot> = {}
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
    error: null,
    ...overrides
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
  private startedSession: Session | undefined;
  public startCalls = 0;
  public stopCalls = 0;

  constructor(
    private readonly nodeId: string,
    private readonly onTerminal: (session: Session) => void,
    private readonly initialSession?: Session,
    private readonly startDelayMs = 0
  ) {
    if (initialSession) {
      this.startedSession = initialSession;
    }
  }

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
    if (this.startDelayMs > 0) {
      await new Promise<void>((resolve) => {
        setTimeout(resolve, this.startDelayMs);
      });
    }
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
    this.startedSession = session;
    return session;
  }

  async stopSession(sessionId: string): Promise<Session> {
    this.stopCalls += 1;
    const session = this.startedSession;
    if (!session || session.id !== sessionId) {
      throw new Error(`Missing session ${sessionId}`);
    }
    const stopped: Session = {
      ...session,
      status: 'stopped',
      endedAt: new Date().toISOString()
    };
    this.sessions.set(sessionId, stopped);
    this.startedSession = stopped;
    this.onTerminal(stopped);
    return stopped;
  }

  async sendCommand(sessionId: string, _command: string): Promise<void> {
    if (!this.startedSession || this.startedSession.id !== sessionId) {
      throw new Error(`Missing session ${sessionId}`);
    }
  }

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
        transport = new FakeTransport(node.nodeId, context.onTerminal, context.session);
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
      branch: 'feature/pinned-route',
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
        branch: 'feature/bad-pin',
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

    await restartedCoordinator.sendCommand(first.id, 'status');

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

test('fleet dispatch can stop a remote lease after a coordinator restart', async () => {
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
      requestId: 'dispatch-stop-restart',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve'
    });

    mockNode.sessions = [first];

    const restartedTransportMap = new Map<string, FakeTransport>();
    const restartedCoordinator = createCoordinatorHarness({
      leaseStorePath,
      registryService,
      coordinatorNodeId: coordinatorIdentity.node_id,
      coordinatorUrl: coordinatorIdentity.advertised_url,
      transportMap: restartedTransportMap,
      published
    });

    const retry = await restartedCoordinator.startSession({
      requestId: 'dispatch-stop-restart',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      mode: 'improve'
    });

    assert.equal(retry.id, first.id);

    const stopped = await restartedCoordinator.stopSession(first.id);
    assert.equal(stopped.status, 'stopped');
    assert.equal(restartedTransportMap.get('worker-1')?.stopCalls, 1);
  } finally {
    await mockNode.close();
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});

test('fleet dispatch deduplicates concurrent starts with the same request id', async () => {
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('coordinator-1', 'http://127.0.0.1:9999', 10, 0, 0)
      ];
    }
  } as unknown as RegistryService;

  transportMap.set(
    'coordinator-1',
    new FakeTransport('coordinator-1', () => {}, undefined, 50)
  );

  const coordinator = createCoordinatorHarness({
    leaseStorePath,
    registryService,
    coordinatorNodeId: 'coordinator-1',
    coordinatorUrl: 'http://127.0.0.1:9999',
    transportMap,
    published
  });

  try {
    const options = {
      requestId: 'dispatch-race',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      branch: 'feature/request-dedupe',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-4.1'
    } as const;

    const first = coordinator.startSession(options);
    const second = coordinator.startSession(options);

    await new Promise<void>((resolve) => setTimeout(resolve, 25));
    assert.equal(transportMap.get('coordinator-1')?.startCalls, 1);

    const [firstSession, secondSession] = await Promise.all([first, second]);
    assert.equal(firstSession.id, secondSession.id);
    assert.equal(firstSession.nodeId, 'coordinator-1');
    assert.equal(
      published.filter((message) => message.type === 'session.started').length,
      0
    );
  } finally {
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});

test('fleet dispatch deduplicates concurrent starts for the same work identity across request ids', async () => {
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('worker-1', 'http://127.0.0.1:9998', 15, 0, 0, {
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-4.1'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ],
          availability: [
            {
              backend: 'codex',
              model: 'gpt-4.1',
              quota_pool: null,
              eligible_now: true,
              reason: null,
              unavailable_until: null,
              source: 'test',
              last_error_summary: null,
              observed_at: new Date().toISOString(),
              scope: 'model_specific'
            }
          ]
        })
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
    const firstOptions = {
      requestId: 'dispatch-work-1',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      branch: 'feature/work-dedupe',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-4.1'
    } as const;
    const secondOptions = {
      ...firstOptions,
      requestId: 'dispatch-work-2'
    } as const;

    const first = coordinator.startSession(firstOptions);
    const second = coordinator.startSession(secondOptions);
    const [firstSession, secondSession] = await Promise.all([first, second]);

    assert.equal(firstSession.id, secondSession.id);
    assert.equal(transportMap.get('worker-1')?.startCalls, 1);
    assert.equal(
      published.filter((message) => message.type === 'session.started').length,
      1
    );
  } finally {
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});

test('fleet dispatch allows redispatch after terminal completion and keeps backend/model scoped work identities distinct', async () => {
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('coordinator-1', 'http://127.0.0.1:9999', 10, 0, 0, {
          backend_configured: { codex: true, claude: true },
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-4.1'],
              executable_configured: true,
              isolated_state_configured: true
            },
            {
              backend_instance: 'claude-main',
              runner_kind: 'claude',
              logical_backend: 'claude',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['sonnet-4.1'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ]
        })
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
    const first = await coordinator.startSession({
      requestId: 'dispatch-terminal-1',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      branch: 'feature/terminal-redispatch',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-4.1'
    });

    const stopped = await coordinator.stopSession(first.id);
    assert.equal(stopped.status, 'stopped');

    const second = await coordinator.startSession({
      requestId: 'dispatch-terminal-2',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      branch: 'feature/terminal-redispatch',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-4.1'
    });

    assert.notEqual(second.id, first.id);
    assert.equal(second.status, 'running');

    const modelScoped = await coordinator.startSession({
      requestId: 'dispatch-model-scope',
      profile: 'gah',
      providerKind: 'claude',
      instanceId: 'claude-0',
      repo: 'owner/repo',
      branch: 'feature/terminal-redispatch',
      mode: 'improve',
      backend: 'claude',
      model: 'sonnet-4.1'
    });

    assert.notEqual(modelScoped.id, second.id);
    assert.notEqual(modelScoped.id, first.id);
    assert.equal(modelScoped.backend, 'claude');
    assert.equal(modelScoped.model, 'sonnet-4.1');
  } finally {
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});

test('fleet dispatch skips unsupported, unavailable, and saturated nodes, and marks a partitioned lease uncertain', async () => {
  const leaseDir = mkdtempSync(resolve(tmpdir(), 'gah-fleet-'));
  const leaseStorePath = resolve(leaseDir, 'dispatch-leases.json');
  const published: PublishedMessage[] = [];
  const transportMap = new Map<string, FakeTransport>();
  const unreachableWorkerUrl = 'http://127.0.0.1:65535';

  const registryService = {
    async getNodeObservations() {
      return [
        makeSnapshot('blocked-availability', 'http://127.0.0.1:9997', 5, 0, 0, {
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-4.1'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ],
          availability: [
            {
              backend: 'codex',
              model: 'gpt-4.1',
              quota_pool: null,
              eligible_now: false,
              reason: 'quota exhausted',
              unavailable_until: null,
              source: 'test',
              last_error_summary: 'quota exhausted',
              observed_at: new Date().toISOString(),
              scope: 'model_specific'
            }
          ]
        }),
        makeSnapshot('blocked-model', 'http://127.0.0.1:9996', 5, 0, 0, {
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-4.1'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ]
        }),
        makeSnapshot('saturated-node', 'http://127.0.0.1:9995', 10, 1, 0, {
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-5.4'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ]
        }),
        makeSnapshot('worker-1', unreachableWorkerUrl, 10, 0, 0, {
          backend_instances: [
            {
              backend_instance: 'codex-main',
              runner_kind: 'codex',
              logical_backend: 'codex',
              account_label: null,
              auth_source_label: null,
              quota_pool: null,
              supported_models: ['gpt-5.4'],
              executable_configured: true,
              isolated_state_configured: true
            }
          ],
          availability: [
            {
              backend: 'codex',
              model: 'gpt-5.4',
              quota_pool: null,
              eligible_now: true,
              reason: null,
              unavailable_until: null,
              source: 'test',
              last_error_summary: null,
              observed_at: new Date().toISOString(),
              scope: 'model_specific'
            }
          ]
        })
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
      requestId: 'dispatch-policy',
      profile: 'gah',
      providerKind: 'codex',
      instanceId: 'codex-0',
      repo: 'owner/repo',
      branch: 'feature/policy-route',
      mode: 'improve',
      backend: 'codex',
      model: 'gpt-5.4'
    });

    assert.equal(routed.nodeId, 'worker-1');
    assert.equal(transportMap.get('worker-1')?.startCalls, 1);
    assert.equal(transportMap.get('blocked-availability')?.startCalls ?? 0, 0);
    assert.equal(transportMap.get('blocked-model')?.startCalls ?? 0, 0);
    assert.equal(transportMap.get('saturated-node')?.startCalls ?? 0, 0);

    await coordinator.reconcileLeases('gah');
    const reconciled = coordinator.getSession(routed.id);
    assert.equal(reconciled?.leaseState, 'uncertain_reconciling');
  } finally {
    try {
      unlinkSync(leaseStorePath);
    } catch {
      // ignore cleanup failures
    }
  }
});
