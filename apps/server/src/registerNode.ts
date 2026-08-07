/**
 * Registers this node against a central node's registry (issue #881),
 * replacing a hand-built curl call with a repeatable one. Run FROM the
 * node being registered -- see registerNodeCli.ts for the CLI entrypoint.
 *
 * Identity (node_id/display_name/advertised_url/version/schema_digest)
 * comes from this node's own `GET /health`, which already exposes
 * getCoordinatorIdentity()'s output -- reusing it instead of re-deriving
 * identity here avoids a second generation path drifting from that one.
 */

import type { RegisteredNode } from '@git-agent-harness/contracts';

export interface RegisterNodeOptions {
  centralUrl: string;
  selfUrl?: string;
  transportMode: RegisteredNode['transport_mode'];
  secretRef: string;
  labels?: string[];
  /** Profiles this node will dispatch (issue #882): the central claims API
   * refuses to grant a lease for a profile the node never declared here. */
  profiles?: string[];
  token?: string;
}

export interface RegisterNodeResult {
  node_id: string;
  display_name: string;
  warnings: string[];
}

interface SelfIdentity {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
}

export async function registerNode(opts: RegisterNodeOptions): Promise<RegisterNodeResult> {
  const selfUrl = opts.selfUrl ?? 'http://127.0.0.1:3773';

  const healthRes = await fetch(`${selfUrl}/health`);
  if (!healthRes.ok) {
    throw new Error(`Failed to read own identity from ${selfUrl}/health: HTTP ${healthRes.status}`);
  }
  const identity = (await healthRes.json()) as SelfIdentity;

  const payload: RegisteredNode = {
    node_id: identity.node_id,
    display_name: identity.display_name,
    advertised_url: identity.advertised_url,
    version: identity.version,
    schema_digest: identity.schema_digest,
    transport_mode: opts.transportMode,
    secret_ref: opts.secretRef,
    labels: opts.labels,
    profiles: opts.profiles
  };

  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (opts.token) headers.Authorization = `Bearer ${opts.token}`;

  const registerRes = await fetch(`${opts.centralUrl.replace(/\/+$/, '')}/api/registry/nodes`, {
    method: 'POST',
    headers,
    body: JSON.stringify(payload)
  });
  const body = (await registerRes.json().catch(() => ({}))) as { warnings?: string[]; message?: string };
  if (!registerRes.ok) {
    throw new Error(`Registration failed: HTTP ${registerRes.status} ${body.message ?? JSON.stringify(body)}`);
  }

  return {
    node_id: payload.node_id,
    display_name: payload.display_name,
    warnings: body.warnings ?? []
  };
}
