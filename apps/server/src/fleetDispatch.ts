import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import WebSocket from 'ws';
import { generateRequestId, GAHError } from '@git-agent-harness/shared';
import type {
  ClientCapabilities,
  DispatchLeaseState,
  NodeObservationSnapshot,
  ServerMessage,
  Session
} from '@git-agent-harness/contracts';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { getSessionManager, type SessionOptions } from './sessions/SessionManager.js';
import type { RegistryService } from './registryService.js';

type PushBusLike = {
  publish(message: ServerMessage): void;
};

type RoutedSessionOptions = SessionOptions & {
  requestId?: string;
  nodeId?: string;
  coordinatorNodeId?: string;
};

type FleetDispatchDeps = {
  registryService: RegistryService;
  pushBus: PushBusLike;
  coordinatorIdentity?: ReturnType<typeof getCoordinatorIdentity>;
  localSessionManager?: ReturnType<typeof getSessionManager>;
  leaseStorePath?: string;
  transportFactory?: (node: NodeSelection, context: {
    coordinatorNodeId: string;
    pushBus: PushBusLike;
    onTerminal: (session: Session) => void;
    localSessionManager: ReturnType<typeof getSessionManager>;
  }) => NodeDispatchTransport;
};

type LeaseRecord = {
  requestId: string;
  nodeId: string;
  nodeUrl: string;
  session: Session;
  state: DispatchLeaseState;
  createdAt: string;
  updatedAt: string;
  coordinatorNodeId: string;
  pinnedNodeId?: string;
};

type NodeSelection = {
  nodeId: string;
  advertisedUrl: string;
  snapshot: NodeObservationSnapshot | null;
  isLocal: boolean;
};

interface NodeDispatchTransport {
  startSession(options: RoutedSessionOptions): Promise<Session>;
  stopSession(sessionId: string): Promise<Session>;
  sendCommand(sessionId: string, command: string): Promise<void>;
  getSession(sessionId: string): Session | undefined;
  getSessions(): Session[];
  close(): Promise<void>;
}

const TERMINAL_LEASE_TTL_MS = 60 * 60 * 1000;
const RECONCILE_TIMEOUT_MS = 5_000;

function nowIso(): string {
  return new Date().toISOString();
}

function decorateSession(session: Session, nodeId: string, state: DispatchLeaseState): Session {
  return {
    ...session,
    nodeId,
    leaseState: state
  };
}

function sessionIsTerminal(session: Session): boolean {
  return session.status === 'stopped' || session.status === 'error';
}

function toWsUrl(advertisedUrl: string): string {
  const url = new URL('/ws', advertisedUrl);
  if (url.protocol === 'https:') {
    url.protocol = 'wss:';
  } else if (url.protocol === 'http:') {
    url.protocol = 'ws:';
  }
  return url.toString();
}

function leaseStorePath(defaultPath: string): string {
  if (process.env.GAH_DISPATCH_LEASES_PATH) {
    return process.env.GAH_DISPATCH_LEASES_PATH;
  }
  return defaultPath;
}

class LeaseStore {
  private leases = new Map<string, LeaseRecord>();

  constructor(private readonly path: string) {
    this.load();
  }

  private load(): void {
    if (!existsSync(this.path)) {
      return;
    }
    try {
      const payload = JSON.parse(readFileSync(this.path, 'utf8')) as { leases?: LeaseRecord[] };
      if (Array.isArray(payload.leases)) {
        for (const lease of payload.leases) {
          if (lease?.requestId && lease?.session?.id && lease?.nodeId && lease?.nodeUrl) {
            this.leases.set(lease.requestId, lease);
          }
        }
      }
    } catch (error) {
      console.warn(`warning: failed to load dispatch lease store ${this.path}:`, error);
    }
  }

  private save(): void {
    const dir = dirname(this.path);
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
    const payload = {
      leases: Array.from(this.leases.values())
    };
    writeFileSync(this.path, JSON.stringify(payload, null, 2));
  }

  getByRequestId(requestId: string): LeaseRecord | undefined {
    return this.leases.get(requestId);
  }

  getBySessionId(sessionId: string): LeaseRecord | undefined {
    for (const lease of this.leases.values()) {
      if (lease.session.id === sessionId) {
        return lease;
      }
    }
    return undefined;
  }

  values(): LeaseRecord[] {
    return Array.from(this.leases.values());
  }

  upsert(record: LeaseRecord): void {
    this.leases.set(record.requestId, record);
    this.save();
  }

  delete(requestId: string): void {
    if (this.leases.delete(requestId)) {
      this.save();
    }
  }

  cleanup(): void {
    const now = Date.now();
    let changed = false;
    for (const [requestId, lease] of this.leases) {
      if (lease.state === 'terminal') {
        const endedAt = Date.parse(lease.session.endedAt ?? lease.updatedAt);
        if (!Number.isNaN(endedAt) && now - endedAt > TERMINAL_LEASE_TTL_MS) {
          this.leases.delete(requestId);
          changed = true;
        }
      }
    }
    if (changed) {
      this.save();
    }
  }
}

class LocalNodeTransport implements NodeDispatchTransport {
  constructor(private readonly localSessionManager: ReturnType<typeof getSessionManager>) {}

  async startSession(options: RoutedSessionOptions): Promise<Session> {
    const session = await this.localSessionManager.startSession(options);
    return session;
  }

  async stopSession(sessionId: string): Promise<Session> {
    return this.localSessionManager.stopSession(sessionId);
  }

  async sendCommand(sessionId: string, command: string): Promise<void> {
    await this.localSessionManager.sendCommand(sessionId, command);
  }

  getSession(sessionId: string): Session | undefined {
    return this.localSessionManager.getSession(sessionId);
  }

  getSessions(): Session[] {
    return this.localSessionManager.getAllSessions();
  }

  async close(): Promise<void> {}
}

class RemoteNodeTransport implements NodeDispatchTransport {
  private socket: WebSocket | null = null;
  private profile = 'gah';
  private startedSession: Session | undefined;
  private startResolver:
    | { resolve: (session: Session) => void; reject: (error: Error) => void }
    | undefined;
  private stopResolver:
    | { resolve: (session: Session) => void; reject: (error: Error) => void }
    | undefined;
  private suppressNextStopBroadcast = false;
  private closed = false;

  constructor(
    private readonly node: NodeSelection,
    private readonly pushBus: PushBusLike,
    private readonly coordinatorNodeId: string,
    private readonly onTerminal: (session: Session) => void
  ) {}

  async startSession(options: RoutedSessionOptions): Promise<Session> {
    if (this.startedSession) {
      return this.startedSession;
    }
    const socket = await this.connect(options.profile);
    const session = await new Promise<Session>((resolve, reject) => {
      this.startResolver = { resolve, reject };
      socket.send(
        JSON.stringify({
          type: 'session.start',
          requestId: options.requestId ?? generateRequestId(),
          coordinatorNodeId: this.coordinatorNodeId,
          nodeId: this.node.nodeId,
          profile: options.profile,
          providerKind: options.providerKind,
          instanceId: options.instanceId,
          repo: options.repo,
          branch: options.branch,
          target: options.target,
          mode: options.mode,
          backend: options.backend,
          model: options.model,
          budget: options.budget,
          dryRun: options.dryRun,
          retries: options.retries,
          allowDraftFail: options.allowDraftFail,
          prod: options.prod,
          allowUnknownRedBaseline: options.allowUnknownRedBaseline,
          escalate: options.escalate
        })
      );
      setTimeout(() => {
        if (this.startResolver) {
          this.startResolver.reject(new Error(`Timed out waiting for remote session start on ${this.node.nodeId}`));
          this.startResolver = undefined;
        }
      }, RECONCILE_TIMEOUT_MS).unref?.();
    });
    this.startedSession = decorateSession(session, this.node.nodeId, 'running');
    return this.startedSession;
  }

  async stopSession(sessionId: string): Promise<Session> {
    const session = this.startedSession;
    if (!session || session.id !== sessionId) {
      throw new GAHError(`Session ${sessionId} not found on node ${this.node.nodeId}`, 'SESSION_NOT_FOUND');
    }
    const socket = await this.connect();
    const stopped = await new Promise<Session>((resolve, reject) => {
      this.stopResolver = { resolve, reject };
      this.suppressNextStopBroadcast = true;
      socket.send(
        JSON.stringify({
          type: 'session.stop',
          requestId: generateRequestId(),
          sessionId
        })
      );
      setTimeout(() => {
        if (this.stopResolver) {
          this.stopResolver.reject(new Error(`Timed out waiting for remote session stop on ${this.node.nodeId}`));
          this.stopResolver = undefined;
        }
      }, RECONCILE_TIMEOUT_MS).unref?.();
    });
    this.startedSession = decorateSession(stopped, this.node.nodeId, 'terminal');
    return this.startedSession;
  }

  async sendCommand(sessionId: string, command: string): Promise<void> {
    if (!this.startedSession || this.startedSession.id !== sessionId) {
      throw new GAHError(`Session ${sessionId} not found on node ${this.node.nodeId}`, 'SESSION_NOT_FOUND');
    }
    const socket = await this.connect();
    socket.send(
      JSON.stringify({
        type: 'session.sendCommand',
        requestId: generateRequestId(),
        sessionId,
        command
      })
    );
  }

  getSession(sessionId: string): Session | undefined {
    return this.startedSession?.id === sessionId ? this.startedSession : undefined;
  }

  getSessions(): Session[] {
    return this.startedSession ? [this.startedSession] : [];
  }

  async close(): Promise<void> {
    this.closed = true;
    if (this.socket && this.socket.readyState === WebSocket.OPEN) {
      this.socket.close();
    }
  }

  private async connect(profileHint?: string): Promise<WebSocket> {
    if (this.socket && this.socket.readyState === WebSocket.OPEN) {
      return this.socket;
    }
    if (profileHint) {
      this.profile = profileHint;
    }
    const socket = new WebSocket(toWsUrl(this.node.advertisedUrl));
    this.socket = socket;
    await new Promise<void>((resolve, reject) => {
      const handleOpen = () => {
        socket.send(
          JSON.stringify({
            type: 'client.hello',
            clientVersion: '0.1.0',
            profile: this.profile,
            capabilities: {
              supportsTerminal: false,
              supportsNotifications: true,
              version: '0.1.0'
            } satisfies ClientCapabilities
          })
        );
        resolve();
      };
      const handleError = (error: Error) => reject(error);
      socket.once('open', handleOpen);
      socket.once('error', handleError);
      socket.on('message', (data: WebSocket.RawData) => {
        this.handleMessage(data);
      });
      socket.on('close', () => {
        if (!this.closed && this.startedSession && !sessionIsTerminal(this.startedSession)) {
          const session = decorateSession(
            {
              ...this.startedSession,
              status: 'error',
              endedAt: nowIso(),
              error: `Connection to node ${this.node.nodeId} closed`
            },
            this.node.nodeId,
            'terminal'
          );
          this.pushBus.publish({
            type: 'session.stopped',
            session
          });
          this.onTerminal(session);
        }
      });
    });
    return socket;
  }

  private handleMessage(data: WebSocket.RawData): void {
    let message: ServerMessage;
    try {
      message = JSON.parse(data.toString()) as ServerMessage;
    } catch {
      return;
    }

    if (message.type === 'session.started') {
      if (this.startResolver) {
        const session = decorateSession(message.session, this.node.nodeId, 'running');
        this.startedSession = session;
        this.startResolver.resolve(session);
        this.startResolver = undefined;
      } else if (this.startedSession?.id === message.session.id) {
        this.startedSession = decorateSession(message.session, this.node.nodeId, 'running');
      }
      return;
    }

    if (message.type === 'session.stopped') {
      const session = decorateSession(message.session, this.node.nodeId, 'terminal');
      this.startedSession = session;
      this.onTerminal(session);
      if (this.stopResolver) {
        this.stopResolver.resolve(session);
        this.stopResolver = undefined;
        return;
      }
      if (this.suppressNextStopBroadcast) {
        this.suppressNextStopBroadcast = false;
        return;
      }
      this.pushBus.publish({
        type: 'session.stopped',
        session
      });
      return;
    }

    if (message.type === 'session.stdout' || message.type === 'session.stderr') {
      if (this.startedSession?.id !== message.sessionId) {
        return;
      }
      this.pushBus.publish(message);
      return;
    }

    if (message.type === 'session.status' && this.startedSession?.id === message.session.id) {
      this.startedSession = decorateSession(message.session, this.node.nodeId, this.startedSession.leaseState ?? 'running');
    }
  }
}

function defaultTransportFactory(
  node: NodeSelection,
  context: {
    coordinatorNodeId: string;
    pushBus: PushBusLike;
    onTerminal: (session: Session) => void;
    localSessionManager: ReturnType<typeof getSessionManager>;
  }
): NodeDispatchTransport {
  if (node.isLocal) {
    return new LocalNodeTransport(context.localSessionManager);
  }
  return new RemoteNodeTransport(node, context.pushBus, context.coordinatorNodeId, context.onTerminal);
}

export class FleetDispatchCoordinator {
  private readonly registryService: RegistryService;
  private readonly pushBus: PushBusLike;
  private readonly coordinatorIdentity: ReturnType<typeof getCoordinatorIdentity>;
  private readonly localSessionManager: ReturnType<typeof getSessionManager>;
  private readonly leaseStore: LeaseStore;
  private readonly remoteTransports = new Map<string, NodeDispatchTransport>();
  private readonly localTransport: LocalNodeTransport;
  private readonly transportFactory: NonNullable<FleetDispatchDeps['transportFactory']>;

  constructor(deps: FleetDispatchDeps) {
    this.registryService = deps.registryService;
    this.pushBus = deps.pushBus;
    this.coordinatorIdentity = deps.coordinatorIdentity ?? getCoordinatorIdentity();
    this.localSessionManager = deps.localSessionManager ?? getSessionManager();
    this.leaseStore = new LeaseStore(
      deps.leaseStorePath ?? leaseStorePath('./config/dispatch-leases.json')
    );
    this.localTransport = new LocalNodeTransport(this.localSessionManager);
    this.transportFactory = deps.transportFactory ?? defaultTransportFactory;
  }

  async startSession(options: RoutedSessionOptions): Promise<Session> {
    await this.reconcileLeases(options.profile);
    const requestId = options.requestId;
    if (requestId) {
      const existing = this.leaseStore.getByRequestId(requestId);
      if (existing) {
        return existing.session;
      }
    }

    const target = await this.selectNode(options);
    if (target.nodeId === this.coordinatorIdentity.node_id) {
      const transport = this.transportFactory(target, {
        coordinatorNodeId: this.coordinatorIdentity.node_id,
        pushBus: this.pushBus,
        onTerminal: (session) => this.updateLeaseForSession(session),
        localSessionManager: this.localSessionManager
      });
      const session = await transport.startSession(options);
      const routed = decorateSession(session, this.coordinatorIdentity.node_id, 'running');
      this.remoteTransports.set(routed.id, transport);
      this.recordLease({
        requestId: requestId ?? routed.id,
        nodeId: this.coordinatorIdentity.node_id,
        nodeUrl: this.coordinatorIdentity.advertised_url,
        session: routed,
        state: 'running',
        createdAt: nowIso(),
        updatedAt: nowIso(),
        coordinatorNodeId: options.coordinatorNodeId ?? this.coordinatorIdentity.node_id,
        pinnedNodeId: options.nodeId
      });
      return routed;
    }

    const transport = this.transportFactory(target, {
      coordinatorNodeId: this.coordinatorIdentity.node_id,
      pushBus: this.pushBus,
      onTerminal: (session) => this.updateLeaseForSession(session),
      localSessionManager: this.localSessionManager
    });
    const session = await transport.startSession({
      ...options,
      nodeId: target.nodeId,
      coordinatorNodeId: this.coordinatorIdentity.node_id
    });
    this.remoteTransports.set(session.id, transport);
    const routed = decorateSession(session, target.nodeId, 'running');
    this.recordLease({
      requestId: requestId ?? routed.id,
      nodeId: target.nodeId,
      nodeUrl: target.advertisedUrl,
      session: routed,
      state: 'running',
      createdAt: nowIso(),
      updatedAt: nowIso(),
      coordinatorNodeId: this.coordinatorIdentity.node_id,
      pinnedNodeId: options.nodeId
    });
    this.pushBus.publish({
      type: 'session.started',
      session: routed
    });
    return routed;
  }

  async stopSession(sessionId: string): Promise<Session> {
    const local = this.localTransport.getSession(sessionId);
    if (local) {
      const session = await this.localTransport.stopSession(sessionId);
      this.updateLeaseForSession(session);
      return decorateSession(session, this.coordinatorIdentity.node_id, 'terminal');
    }

    const lease = this.leaseStore.getBySessionId(sessionId);
    if (!lease) {
      throw new GAHError(`Session ${sessionId} not found`, 'SESSION_NOT_FOUND');
    }

    const transport = this.remoteTransports.get(sessionId) ?? this.transportFactory(
      this.resolveNodeSelectionByLease(lease),
      {
        coordinatorNodeId: this.coordinatorIdentity.node_id,
        pushBus: this.pushBus,
        onTerminal: (session) => this.updateLeaseForSession(session),
        localSessionManager: this.localSessionManager
      }
    );
    const session = await transport.stopSession(sessionId);
    this.remoteTransports.set(sessionId, transport);
    const routed = decorateSession(session, lease.nodeId, 'terminal');
    this.recordLease({
      ...lease,
      session: routed,
      state: 'terminal',
      updatedAt: nowIso()
    });
    this.pushBus.publish({
      type: 'session.stopped',
      session: routed
    });
    return routed;
  }

  async sendCommand(sessionId: string, command: string): Promise<void> {
    const local = this.localTransport.getSession(sessionId);
    if (local) {
      await this.localTransport.sendCommand(sessionId, command);
      return;
    }

    const lease = this.leaseStore.getBySessionId(sessionId);
    if (!lease) {
      throw new GAHError(`Session ${sessionId} not found`, 'SESSION_NOT_FOUND');
    }
    const transport = this.remoteTransports.get(sessionId) ?? this.transportFactory(
      this.resolveNodeSelectionByLease(lease),
      {
        coordinatorNodeId: this.coordinatorIdentity.node_id,
        pushBus: this.pushBus,
        onTerminal: (session) => this.updateLeaseForSession(session),
        localSessionManager: this.localSessionManager
      }
    );
    await transport.sendCommand(sessionId, command);
    this.remoteTransports.set(sessionId, transport);
  }

  getSession(sessionId: string): Session | undefined {
    return this.localTransport.getSession(sessionId)
      ?? this.leaseStore.getBySessionId(sessionId)?.session;
  }

  getAllSessions(): Session[] {
    const sessions = [...this.localTransport.getSessions(), ...this.leaseStore.values().map((lease) => lease.session)];
    const seen = new Set<string>();
    return sessions.filter((session) => {
      if (seen.has(session.id)) {
        return false;
      }
      seen.add(session.id);
      return true;
    });
  }

  async reconcileLeases(profile: string): Promise<void> {
    const leases = this.leaseStore.values();
    if (leases.length === 0) {
      return;
    }

    for (const lease of leases) {
      if (lease.state === 'terminal') {
        continue;
      }
      const local = lease.nodeId === this.coordinatorIdentity.node_id
        ? this.localTransport.getSession(lease.session.id)
        : undefined;
      if (local) {
        const decorated = decorateSession(local, this.coordinatorIdentity.node_id, sessionIsTerminal(local) ? 'terminal' : 'running');
        this.recordLease({
          ...lease,
          session: decorated,
          state: sessionIsTerminal(local) ? 'terminal' : 'running',
          updatedAt: nowIso()
        });
        continue;
      }
      const probe = await this.probeNodeSessions(lease.nodeId, lease.nodeUrl, profile);
      if (!probe) {
        this.recordLease({
          ...lease,
          state: 'uncertain_reconciling',
          updatedAt: nowIso()
        });
        continue;
      }
      const matched = probe.sessions.find((session) => session.id === lease.session.id);
      if (!matched) {
        this.recordLease({
          ...lease,
          state: 'expired',
          updatedAt: nowIso()
        });
        continue;
      }
      const nextState: DispatchLeaseState = sessionIsTerminal(matched) ? 'terminal' : 'running';
      this.recordLease({
        ...lease,
        session: decorateSession(matched, lease.nodeId, nextState),
        state: nextState,
        updatedAt: nowIso()
      });
    }

    this.leaseStore.cleanup();
  }

  private recordLease(record: LeaseRecord): void {
    this.leaseStore.upsert(record);
  }

  private updateLeaseForSession(session: Session): void {
    const lease = this.leaseStore.getBySessionId(session.id);
    if (!lease) {
      return;
    }
    const nextState: DispatchLeaseState = sessionIsTerminal(session) ? 'terminal' : 'running';
    this.recordLease({
      ...lease,
      session: decorateSession(session, lease.nodeId, nextState),
      state: nextState,
      updatedAt: nowIso()
    });
  }

  private async selectNode(options: RoutedSessionOptions): Promise<NodeSelection> {
    const nodes = await this.registryService.getNodeObservations(options.profile);
    const coordinatorId = this.coordinatorIdentity.node_id;
    const candidates: NodeSelection[] = [];

    for (const snapshot of nodes) {
      if (!this.isSnapshotEligible(snapshot, options)) {
        continue;
      }
      candidates.push({
        nodeId: snapshot.node_id,
        advertisedUrl: snapshot.advertised_url,
        snapshot,
        isLocal: snapshot.node_id === coordinatorId
      });
    }

    if (!candidates.some((candidate) => candidate.nodeId === coordinatorId)) {
      candidates.push({
        nodeId: coordinatorId,
        advertisedUrl: this.coordinatorIdentity.advertised_url,
        snapshot: null,
        isLocal: true
      });
    }

    const pin = options.nodeId;
    if (pin) {
      const pinned = candidates.find((candidate) => candidate.nodeId === pin);
      if (!pinned) {
        throw new GAHError(`Pinned node ${pin} is not healthy or not registered for profile ${options.profile}`, 'NODE_PINNING_FAILED');
      }
      return pinned;
    }

    candidates.sort((left, right) => this.compareNodeSelection(left, right));
    const selected = candidates[0];
    if (!selected) {
      throw new GAHError(`No healthy node available for profile ${options.profile}`, 'NODE_UNAVAILABLE');
    }
    return selected;
  }

  private compareNodeSelection(left: NodeSelection, right: NodeSelection): number {
    const leftScore = this.scoreSnapshot(left.snapshot);
    const rightScore = this.scoreSnapshot(right.snapshot);
    if (leftScore !== rightScore) {
      return leftScore - rightScore;
    }
    if (left.nodeId === this.coordinatorIdentity.node_id && right.nodeId !== this.coordinatorIdentity.node_id) {
      return -1;
    }
    if (right.nodeId === this.coordinatorIdentity.node_id && left.nodeId !== this.coordinatorIdentity.node_id) {
      return 1;
    }
    return left.nodeId.localeCompare(right.nodeId);
  }

  private scoreSnapshot(snapshot: NodeObservationSnapshot | null): number {
    if (!snapshot) {
      return 0;
    }
    const cpu = snapshot.resource_pressure.cpu_percent ?? 100;
    const disk = snapshot.resource_pressure.disk_percent ?? 100;
    const rss = snapshot.resource_pressure.rss_bytes ?? Number.MAX_SAFE_INTEGER;
    const active = snapshot.active_work.length + snapshot.active_claims.length;
    return active * 1_000_000 + cpu * 10_000 + disk * 100 + Math.min(rss / 1_000_000, 99);
  }

  private isSnapshotEligible(snapshot: NodeObservationSnapshot, options: RoutedSessionOptions): boolean {
    if (snapshot.state !== 'healthy') {
      return false;
    }
    if (snapshot.profile && snapshot.profile !== options.profile) {
      return false;
    }
    if (snapshot.profiles.length > 0 && !snapshot.profiles.includes(options.profile)) {
      return false;
    }
    if (options.backend && snapshot.backend_configured[options.backend] === false) {
      return false;
    }
    return true;
  }

  private resolveNodeSelectionByLease(lease: LeaseRecord): NodeSelection {
    if (lease.nodeId === this.coordinatorIdentity.node_id) {
      return {
        nodeId: lease.nodeId,
        advertisedUrl: this.coordinatorIdentity.advertised_url,
        snapshot: null,
        isLocal: true
      };
    }
    return {
      nodeId: lease.nodeId,
      advertisedUrl: lease.nodeUrl,
      snapshot: null,
      isLocal: false
    };
  }

  private async probeNodeSessions(
    nodeId: string,
    nodeUrl: string,
    profile: string
  ): Promise<{ sessions: Session[] } | null> {
    if (nodeId === this.coordinatorIdentity.node_id) {
      return {
        sessions: this.localTransport.getSessions()
      };
    }
    const socket = new WebSocket(toWsUrl(nodeUrl));
    try {
      const sessions = await new Promise<Session[]>((resolve, reject) => {
        const timer = setTimeout(() => {
          reject(new Error(`Timed out probing node ${nodeId}`));
        }, RECONCILE_TIMEOUT_MS);
        timer.unref?.();
        socket.once('open', () => {
          socket.send(
            JSON.stringify({
              type: 'client.hello',
              clientVersion: '0.1.0',
              profile,
              capabilities: {
                supportsTerminal: false,
                supportsNotifications: true,
                version: '0.1.0'
              } satisfies ClientCapabilities
            })
          );
        });
        socket.on('message', (data: WebSocket.RawData) => {
          try {
            const message = JSON.parse(data.toString()) as ServerMessage;
            if (message.type === 'server.welcome') {
              clearTimeout(timer);
              resolve(message.sessions);
              socket.close();
            }
          } catch {
            // ignore malformed probe messages
          }
        });
        socket.once('error', (error) => {
          clearTimeout(timer);
          reject(error);
        });
      });
      return { sessions };
    } catch {
      socket.close();
      return null;
    }
  }
}

export function createFleetDispatchCoordinator(deps: FleetDispatchDeps): FleetDispatchCoordinator {
  return new FleetDispatchCoordinator(deps);
}
