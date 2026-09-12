/** Resolve conversation ownership separately from the node executing its next
 * turn. Registry observations are reused; querying the selector never polls. */
import type { ChatNodeInfo, ChatSessionSummary } from '@git-agent-harness/contracts';
import type { RegistryService } from './registryService.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { resolveChatProject } from './projectCatalog.js';
import { runProfileList } from './gahCli.js';
import { workerChatConnection } from './remoteChat.js';

let registry: RegistryService | undefined;
let identity = getCoordinatorIdentity;
export function configureChatRouting(service: RegistryService, localIdentity = getCoordinatorIdentity) {
  registry = service;
  identity = localIdentity;
}

export async function chatNodes(profile?: string, backend?: string): Promise<ChatNodeInfo[]> {
  const local = identity();
  const project = profile ? await resolveChatProject(profile) : null;
  const localProfiles = profile && project?.node_id !== local.node_id ? await runProfileList() : [];
  const localEligible = !profile || !!project && (project.node_id === local.node_id || localProfiles.some(candidate => candidate.name === project.name && sameRepository(candidate, project)));
  const nodes: ChatNodeInfo[] = [{ nodeId: local.node_id, displayName: local.display_name, role: 'central', chatCapable: localEligible, eligible: localEligible, reason: localEligible ? null : 'This project has no checkout on central.', state: 'healthy', observedAt: null, lastSeenAt: null }];
  for (const node of registry?.getNodesSummary() ?? []) {
    if (node.node_id === local.node_id) continue;
    const snapshot = registry?.getCachedObservations().find(observation => observation.node_id === node.node_id);
    const observedAt = snapshot?.observed_at ?? node.last_observed_at ?? null;
    const age = observedAt ? Date.now() - Date.parse(observedAt) : Infinity;
    const state = !Number.isFinite(age) || age > 30 * 60_000 ? 'stale' : snapshot?.state ?? node.last_observed_state ?? 'unknown';
    let reason: string | null = null;
    if (profile && (!project || !node.profiles?.includes(project.name))) reason = 'This worker has no declared profile for this project.';
    else if (state !== 'healthy') reason = state === 'stale' ? 'Worker health is stale. Check it in Nodes.' : 'Worker health is unknown or unavailable. Check it in Nodes.';
    else if (backend && !snapshot) reason = 'Backend readiness is unknown. Check this worker in Nodes.';
    else if (backend && snapshot && !snapshot.backend_configured[backend] && !snapshot.backend_instances.some(instance => instance.logical_backend === backend && (instance.executable_resolved ?? instance.executable_configured))) reason = 'The selected backend is not resolved on this worker.';
    else if (backend && snapshot?.availability.some(scope => scope.backend === backend && !scope.eligible_now && scope.scope === 'backend_wide')) reason = 'The selected backend is currently unavailable on this worker.';
    nodes.push({ nodeId: node.node_id, displayName: node.display_name, role: 'worker', chatCapable: reason === null, eligible: reason === null, reason, state, observedAt, profiles: node.profiles, lastSeenAt: snapshot?.last_seen_at ?? node.last_seen_at ?? null });
  }
  return nodes;
}

export async function chatRoute(profile: string, nodeId?: string, backend?: string, requireReady = true) {
  const local = registry ? identity() : null;
  // Keep legacy local chat behavior when callers have not selected a worker.
  if (!profile.startsWith('gah-node:') && (!nodeId || nodeId === local?.node_id)) {
    return { nodeId: local?.node_id, nodeName: local?.display_name, profileName: profile, remote: undefined };
  }
  if (!registry || !local) throw new Error('Worker chat routing is not configured.');
  const project = await resolveChatProject(profile);
  if (!project) throw new Error('The project is no longer in the catalog.');
  const targetId = nodeId ?? project.node_id;
  if (requireReady) {
    const target = (await chatNodes(profile, backend)).find(node => node.nodeId === targetId);
    if (!target?.eligible) throw new Error(target?.reason ?? 'This worker is no longer registered.');
  }
  if (targetId === local.node_id) {
    const match = (await runProfileList()).find(candidate => candidate.name === project.name && sameRepository(candidate, project));
    if (!match) throw new Error('This project has no matching checkout on central.');
    return { nodeId: local.node_id, nodeName: local.display_name, profileName: match.name, remote: undefined };
  }
  const node = registry.getNode(targetId);
  if (!node) throw new Error('This worker is no longer registered.');
  return { nodeId: targetId, nodeName: node.display_name, profileName: project.name, remote: workerChatConnection(registry, targetId, project.name, project) };
}

function sameRepository(a: { repo: string; provider: string; web_url: string | null }, b: { repo: string; provider: string; web_url: string | null }): boolean {
  if (!a.web_url || !b.web_url) return false;
  try { return a.repo === b.repo && a.provider === b.provider && new URL(a.web_url).origin === new URL(b.web_url).origin; }
  catch { return false; }
}

export type ChatRoute = Awaited<ReturnType<typeof chatRoute>>;
export function rememberWorkspace(session: ChatSessionSummary, route: ChatRoute): ChatSessionSummary {
  const workspaces = { ...session.workspaces };
  if (route.nodeId) workspaces[route.nodeId] = { branch: session.branch, worktreePath: session.worktreePath };
  return { ...session, nodeId: route.nodeId, remoteWorkspace: !!route.remote, workspaces, workspaceNodes: Object.keys(workspaces) };
}

export function localChatNodeId(): string | undefined { return registry ? identity().node_id : undefined; }
