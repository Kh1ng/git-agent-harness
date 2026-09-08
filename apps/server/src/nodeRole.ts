import type { RequestHandler } from 'express';
import type { NodeRoleStatus } from '@git-agent-harness/contracts';

/** Fail closed when the CLI cannot identify the host; never guess central on a worker. */
export function validateNodeRole(value: NodeRoleStatus): NodeRoleStatus {
  if (value?.role !== 'central' && value?.role !== 'worker') throw new Error('Cannot determine node role. Update the gah CLI and configure gah status --role.');
  if (value.role === 'worker' && !value.central_url) throw new Error('Worker role requires registry_central_url.');
  if (value.central_url !== null) {
    const url = new URL(value.central_url);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.pathname !== '/' || url.search || url.hash) throw new Error('Worker central URL must be an HTTP(S) origin without credentials or a path.');
  }
  return value;
}

/** Workers expose local execution/readiness, never central stores or orchestration. */
export function workerRouteGuard(node: NodeRoleStatus): RequestHandler {
  return (req, res, next) => {
    if (node.role === 'central') return next();
    const readPaths = ['/health', '/api/info', '/api/status', '/api/doctor', '/api/quota', '/api/report', '/api/report/series', '/api/events', '/api/availability', '/api/profiles', '/api/loop/status'];
    const writePaths = ['/api/worker/projects/import', '/api/dispatch', '/api/availability/clear', '/api/loop/start', '/api/loop/stop'];
    if ((req.method === 'GET' && readPaths.includes(req.path)) || (req.method === 'POST' && writePaths.includes(req.path))) return next();
    res.status(409).json({ error: 'This node is an execution worker. Use the central node for control, memory, and skills.', central_url: node.central_url });
  };
}

/** Children inherit a central-only gateway endpoint; the gateway key never leaves central. */
export function workerMemoryEnvironment(node: NodeRoleStatus, token: string | undefined): Record<string, string> {
  validateNodeRole(node);
  if (node.role !== 'worker') return {};
  if (!token?.trim() || /[\x00-\x1f\x7f]/.test(token)) throw new Error('A worker requires COORDINATOR_TOKEN for its execution API and central memory relay.');
  return { TDAI_GATEWAY_URL: `${node.central_url!.replace(/\/$/, '')}/api/worker-memory`, TDAI_GATEWAY_API_KEY: token };
}
