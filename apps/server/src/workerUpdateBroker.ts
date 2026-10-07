/**
 * Coordinator-side worker update broker (issue #1416). The coordinator
 * tells a worker to update by POSTing to the worker's /api/worker-update
 * (same transport rules as the login repair broker: TLS off-loopback
 * unless GAH_ALLOW_INSECURE_HTTP=1, the node's Bearer secret, fixed error
 * text) and can sweep the whole fleet at once. The worker decides when it
 * is safe to run the update -- a mid-dispatch worker finishes first.
 */
import type {
  FleetUpdateResult,
  NodeSummary,
  RegisteredNode,
  WorkerUpdateStatus
} from '@git-agent-harness/contracts';
import { nodeHeaders } from './registryService.js';

export class WorkerUpdateError extends Error {
  constructor(public readonly status: number, message: string) {
    super(message);
  }
}

/** Structural, so tests inject a stub registry without spinning up the real
 * persistence-backed service. */
export interface WorkerUpdateRegistry {
  getNode(nodeId: string): RegisteredNode | undefined;
  getNodes(): RegisteredNode[];
  getNodesSummary(): NodeSummary[];
}

export interface WorkerUpdateBrokerDeps {
  registry: WorkerUpdateRegistry;
  localNodeId: string;
  fetch?: typeof fetch;
}

export class WorkerUpdateBroker {
  constructor(private readonly deps: WorkerUpdateBrokerDeps) {}

  async start(nodeId: string): Promise<{ started: boolean; status: WorkerUpdateStatus }> {
    const payload = await this.worker(nodeId, { action: 'start' });
    return { started: payload.started, status: payload.status };
  }

  async status(nodeId: string): Promise<WorkerUpdateStatus> {
    return (await this.worker(nodeId, { action: 'status' })).status;
  }

  /** Ask every registered worker to update. One unreachable node never
   * blocks the rest; each row reports its own outcome. */
  async updateAll(): Promise<FleetUpdateResult[]> {
    const nodes = this.deps.registry.getNodes().filter((node) => node.node_id !== this.deps.localNodeId);
    const results: FleetUpdateResult[] = [];
    for (const node of nodes) {
      try {
        const started = await this.start(node.node_id);
        results.push({
          node_id: node.node_id,
          display_name: node.display_name,
          started: started.started,
          status: started.status,
          error: null
        });
      } catch (error) {
        results.push({
          node_id: node.node_id,
          display_name: node.display_name,
          started: false,
          status: null,
          error: error instanceof Error ? error.message : String(error)
        });
      }
    }
    return results;
  }

  /** One auto-update sweep (issue #1416): every node opted in via
   * auto_update that the registry sees behind the coordinator gets asked to
   * update. Driven by a scheduler, not by a dashboard click. */
  async autoUpdateSweep(): Promise<FleetUpdateResult[]> {
    const summaries = this.deps.registry.getNodesSummary();
    const eligible = summaries.filter(
      (summary) =>
        summary.node_id !== this.deps.localNodeId &&
        summary.auto_update === true &&
        (summary.update?.status === 'behind' || summary.update?.status === 'unsupported')
    );
    const results: FleetUpdateResult[] = [];
    for (const summary of eligible) {
      try {
        const started = await this.start(summary.node_id);
        results.push({
          node_id: summary.node_id,
          display_name: summary.display_name,
          started: started.started,
          status: started.status,
          error: null
        });
      } catch (error) {
        results.push({
          node_id: summary.node_id,
          display_name: summary.display_name,
          started: false,
          status: null,
          error: error instanceof Error ? error.message : String(error)
        });
      }
    }
    return results;
  }

  private async worker(nodeId: string, body: Record<string, unknown>): Promise<{ started: boolean; status: WorkerUpdateStatus }> {
    // The coordinator never appears in its own worker registry, so this
    // check comes first: asking central to update "itself" through the
    // fleet is a client bug, not a missing node.
    if (nodeId === this.deps.localNodeId) {
      throw new WorkerUpdateError(400, 'The coordinator updates itself from the Settings page, not through the fleet.');
    }
    const node: RegisteredNode | undefined = this.deps.registry.getNode(nodeId);
    if (!node) throw new WorkerUpdateError(404, 'That node is no longer registered.');
    const endpoint = new URL('/api/worker-update', node.advertised_url);
    if (endpoint.protocol !== 'https:' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1'
      && !['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)) {
      throw new WorkerUpdateError(502, 'Updating this worker requires HTTPS or trusted LAN HTTP.');
    }
    let response: Response;
    try {
      response = await (this.deps.fetch ?? fetch)(endpoint, {
        method: 'POST',
        redirect: 'error',
        signal: AbortSignal.timeout(20_000),
        headers: { ...nodeHeaders(node), 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
    } catch {
      throw new WorkerUpdateError(502, 'The worker could not be reached.');
    }
    const payload = await response.json().catch(() => null) as
      | { started?: unknown; status?: unknown; message?: unknown }
      | null;
    if (!response.ok) {
      const status = response.status === 400 || response.status === 404 || response.status === 409 ? response.status : 502;
      const message =
        typeof payload?.message === 'string' && payload.message.length <= 200
          ? payload.message
          : 'The worker refused the update.';
      throw new WorkerUpdateError(status, message);
    }
    if (typeof payload?.started !== 'boolean' || !payload.status || typeof payload.status !== 'object') {
      throw new WorkerUpdateError(502, 'The worker returned an unexpected update status.');
    }
    return { started: payload.started, status: payload.status as WorkerUpdateStatus };
  }
}
