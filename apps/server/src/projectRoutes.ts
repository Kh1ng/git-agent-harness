import { Router } from 'express';
import rateLimit from 'express-rate-limit';
import type { NodeRoleStatus, ProjectImportData, ProjectImportResult } from '@git-agent-harness/contracts';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { runProfileAdd, runProfileList } from './gahCli.js';
import { addProject, addRemoteProject, importGitProject, listProjects, parseGitUrl, projectProfile, removeProject } from './projectCatalog.js';
import { nodeHeaders, type RegistryService } from './registryService.js';

function importInput(value: Record<string, unknown>): ProjectImportData {
  if (typeof value?.gitUrl !== 'string' || !value.gitUrl.trim()) throw new Error('gitUrl is required');
  for (const key of ['nodeId', 'provider', 'providerApiBase', 'providerProjectId']) {
    if (value[key] !== undefined && (typeof value[key] !== 'string' || !(value[key] as string).trim())) throw new Error(`${key} must be a nonempty string`);
  }
  const input: ProjectImportData = {
    gitUrl: value.gitUrl.trim(), reclone: value.reclone === true,
    ...(value.nodeId ? { nodeId: value.nodeId as string } : {}),
    ...(value.provider ? { provider: value.provider as ProjectImportData['provider'] } : {}),
    ...(value.providerApiBase ? { providerApiBase: value.providerApiBase as string } : {}),
    ...(value.providerProjectId ? { providerProjectId: value.providerProjectId as string } : {})
  };
  const identity = parseGitUrl(input.gitUrl, input);
  if (identity.provider === 'gitlab' && !/^[1-9][0-9]*$/.test(input.providerProjectId || '')) throw new Error('GitLab imports require the numeric project ID from the project overview');
  return input;
}

/** The central catalog changes only after the registered worker confirms its own import. */
export async function importProjectOnNode(input: ProjectImportData, registry: RegistryService, localNodeId: string): Promise<ProjectImportResult> {
  const worker = registry.getNode(input.nodeId!);
  if (!worker) throw new Error('The selected worker is not registered. Add it in Settings, then retry.');
  let headers: Record<string, string>;
  try { headers = nodeHeaders(worker); }
  catch { throw new Error('Cannot resolve the selected worker credential on central. Check its registration.'); }
  // Validate the catalog before remote work so corruption cannot produce an uncatalogued import.
  listProjects([], localNodeId);
  const request = async (path: string, body?: unknown) => {
    try {
      const response = await fetch(new URL(path, worker.advertised_url), {
        method: body ? 'POST' : 'GET', headers: { ...headers, 'Content-Type': 'application/json' },
        ...(body ? { body: JSON.stringify(body) } : {}),
        redirect: 'error', signal: AbortSignal.timeout(body ? 300_000 : 5_000)
      });
      if (response.status === 401 || response.status === 403) throw new Error('Worker rejected its configured credential. Check its registration.');
      if (!response.ok) throw new Error(`Worker import endpoint returned HTTP ${response.status}. Check the worker logs and retry.`);
      return await response.json();
    } catch (error) {
      if (error instanceof TypeError || (error instanceof Error && ['AbortError', 'TimeoutError'].includes(error.name))) {
        throw new Error(body ? 'Worker import connection failed or timed out. Check the worker before retrying; it may have completed the import.' : 'The selected worker is offline or unreachable. No project was imported.');
      }
      throw error;
    }
  };
  const health = await request('/health') as { node_id?: unknown; node?: { role?: unknown }; status?: unknown } | null;
  if (health?.node_id !== worker.node_id || health.node?.role !== 'worker' || health.status !== 'healthy') throw new Error('The selected endpoint is not the expected ready worker. Check its registration.');
  if (registry.getNode(worker.node_id) !== worker) throw new Error('Worker registration changed. Retry the import.');
  const payload = await request('/api/worker/projects/import', { ...input, nodeId: worker.node_id }) as Partial<ProjectImportResult> | null;
  if (payload?.project?.node_id !== worker.node_id) throw new Error('Worker returned a mismatched project owner.');
  const profile = projectProfile(payload.project);
  const expected = parseGitUrl(input.gitUrl, input);
  if (profile.repo !== expected.repo || profile.provider !== expected.provider
    || typeof payload.checkoutPath !== 'string' || payload.checkoutPath !== profile.local_path
    || !payload.checkoutStatus || !['cloned', 'verified', 'recloned'].includes(payload.checkoutStatus)
    || !Array.isArray(payload.detectedLanguages) || !payload.detectedLanguages.every((value: unknown) => typeof value === 'string')
    || !Array.isArray(payload.validationCommands) || !payload.validationCommands.every((value: unknown) => typeof value === 'string')) throw new Error('Worker returned an invalid project import result.');
  if (registry.getNode(worker.node_id) !== worker) throw new Error('Worker registration changed during import. Check the worker, then retry.');
  // Only extend declared profiles; URLs, credentials, and other registration fields stay unchanged.
  registry.registerNode({ ...worker, profiles: [...new Set([...(worker.profiles || []), profile.name])] });
  return {
    project: addRemoteProject(worker.node_id, profile, localNodeId),
    checkoutPath: payload.checkoutPath, checkoutStatus: payload.checkoutStatus,
    detectedLanguages: payload.detectedLanguages, validationCommands: payload.validationCommands
  };
}

/** Catalog control belongs to central; workers expose only the authenticated local import operation. */
export function projectRoutes(options: {
  node: NodeRoleStatus;
  registry: RegistryService;
  localNodeId?: string;
  listProfiles?: typeof runProfileList;
  addProfile?: typeof runProfileAdd;
  importGit?: typeof importGitProject;
}): Router {
  const router = Router();
  const localNodeId = options.localNodeId ?? getCoordinatorIdentity().node_id;
  const listProfiles = options.listProfiles ?? runProfileList;
  const addProfile = options.addProfile ?? runProfileAdd;
  const importGit = options.importGit ?? importGitProject;
  const errorResult = (error: unknown) => ({ error: 'Project operation failed', message: error instanceof Error ? error.message : String(error) });
  router.use(['/projects/import', '/worker/projects/import'], rateLimit({ windowMs: 60_000, limit: 10, standardHeaders: true, legacyHeaders: false }));
  router.post(['/projects/import', '/worker/projects/import'], async (req, res) => {
    const workerOperation = req.path === '/worker/projects/import';
    if (workerOperation !== (options.node.role === 'worker')) return res.status(409).json({ error: 'Use the project import endpoint for this node role.' });
    let input: ProjectImportData;
    try { input = importInput(req.body); }
    catch (error) { return res.status(400).json(errorResult(error)); }
    if (workerOperation && input.nodeId !== localNodeId) return res.status(409).json({ error: 'Import target does not match this worker.' });
    try {
      if (!workerOperation && input.nodeId && input.nodeId !== localNodeId) {
        return res.status(201).json(await importProjectOnNode(input, options.registry, localNodeId));
      }
      if (!workerOperation) listProjects([], localNodeId);
      const imported = await importGit(input, { listProfiles, addProfile });
      const profiles = await listProfiles();
      const profile = profiles.find((candidate) => candidate.name === imported.profileName);
      if (!profile) throw new Error('Imported profile could not be read from this node.');
      const project = workerOperation ? { ...projectProfile(profile), node_id: localNodeId, chat_profile: profile.name } : addProject(imported.profileName, profiles, localNodeId);
      const { profileName: _, ...result } = imported;
      return res.status(201).json({ project, ...result } satisfies ProjectImportResult);
    } catch (error) {
      const result = errorResult(error);
      const conflict = ['uncommitted changes', 'checkout origin', 'managed checkouts'].some((text) => result.message.includes(text));
      return res.status(conflict ? 409 : 502).json(result);
    }
  });
  router.get('/projects', async (_req, res) => {
    try { res.json(listProjects(await listProfiles(), localNodeId)); }
    catch (error) { res.status(502).json(errorResult(error)); }
  });
  router.post('/projects', async (req, res) => {
    const profile = typeof req.body?.profile === 'string' ? req.body.profile.trim() : '';
    if (!profile) return res.status(400).json({ error: 'profile is required' });
    try { return res.status(201).json(addProject(profile, await listProfiles(), localNodeId)); }
    catch (error) { return res.status(400).json(errorResult(error)); }
  });
  router.delete('/projects/:profile', (req, res) => {
    if (req.query.nodeId !== undefined && typeof req.query.nodeId !== 'string') return res.status(400).json({ error: 'nodeId must be a string' });
    try { return res.json({ removed: removeProject(req.params.profile, req.query.nodeId as string | undefined ?? localNodeId, localNodeId) }); }
    catch (error) { return res.status(502).json(errorResult(error)); }
  });
  return router;
}
