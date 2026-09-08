import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer, type Server } from 'node:http';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import type { ProfileSummary, ProjectImportResult, RegisteredNode } from '@git-agent-harness/contracts';
import { authMiddleware } from './authMiddleware.js';
import { COORDINATOR_SCHEMA_DIGEST } from './coordinatorIdentity.js';
import { importGitProject } from './projectCatalog.js';
import { projectRoutes } from './projectRoutes.js';
import { RegistryService } from './registryService.js';
import { workerRouteGuard } from './nodeRole.js';

async function listen(server: Server): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  return `http://127.0.0.1:${address.port}`;
}
const close = (server: Server) => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
const post = (url: string, body: unknown, headers = {}) => fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(body) });

test('central import clones and adds the profile only on its authenticated worker, then records ownership', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-remote-import-'));
  const environment = { GAH_PROJECT_CATALOG_PATH: join(root, 'central-projects.json'), GAH_PROJECTS_ROOT: join(root, 'worker-projects'), XDG_DATA_HOME: join(root, 'worker-data'), COORDINATOR_TOKEN: 'worker-test-token', GAH_ALLOW_INSECURE_HTTP: '1' };
  const saved = Object.fromEntries(Object.keys(environment).map((key) => [key, process.env[key]]));
  Object.assign(process.env, environment);
  const centralRegistry = new RegistryService(join(root, 'registry.json'));
  const source = join(root, 'source');
  execFileSync('git', ['init', '--initial-branch=main', source]);
  writeFileSync(join(source, 'Cargo.toml'), '[package]\nname="example"\nversion="0.1.0"\n');
  execFileSync('git', ['-C', source, 'add', '.']);
  execFileSync('git', ['-C', source, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-m', 'fixture']);
  const gitUrl = 'https://github.com/owner/repository.git';
  const workerProfiles: ProfileSummary[] = [];
  let centralProfileCalls = 0;
  let workerImports = 0;
  let healthNodeId = 'worker';
  let redirectHealth = false;
  let unexpectedRequests = 0;
  const workerRole = { role: 'worker', central_url: 'https://central.test' } as const;
  const worker = express();
  worker.use(express.json());
  worker.use('/api', authMiddleware);
  worker.use(workerRouteGuard(workerRole));
  worker.get('/health', (_req, res) => redirectHealth ? res.redirect('/unexpected') : res.json({ node_id: healthNodeId, node: workerRole, status: 'healthy' }));
  worker.get('/unexpected', (_req, res) => { unexpectedRequests += 1; res.sendStatus(200); });
  worker.use('/api/worker/projects/import', (req, res, next) => {
    if (req.headers.authorization !== 'Bearer worker-test-token') return res.sendStatus(401);
    return next();
  });
  worker.use('/api', projectRoutes({ node: workerRole, registry: new RegistryService(null), localNodeId: 'worker',
    listProfiles: async () => workerProfiles,
    addProfile: async (options) => {
      assert.ok(options.local_path.startsWith(environment.GAH_PROJECTS_ROOT));
      assert.equal(existsSync(environment.GAH_PROJECT_CATALOG_PATH), false, 'central catalog is not written before worker success');
      workerProfiles.push({ name: options.name, display_name: options.display_name || options.name, provider: options.provider, repo: options.repo, repo_id: options.repo_id || options.name,
        local_path: options.local_path, worktree_base: join(root, 'worker-worktrees'), web_url: gitUrl.replace(/\.git$/, ''),
        max_parallel_workers: null, max_open_managed_mrs: 1, manager_wake_autonomy: null, validation_timeout_seconds: 300 });
    },
    importGit: (input, dependencies) => {
      workerImports += 1;
      return importGitProject(input, { ...dependencies!, git: async (args) => {
        const effective = args.map((value) => value === gitUrl ? source : value);
        const output = execFileSync('git', effective, { encoding: 'utf8' }).trim();
        if (args[0] === 'clone') execFileSync('git', ['-C', args.at(-1)!, 'remote', 'set-url', 'origin', gitUrl]);
        return output;
      } });
    }
  }));
  const workerServer = createServer(worker);
  const workerUrl = await listen(workerServer);
  const registration: RegisteredNode = { node_id: 'worker', display_name: 'Windows worker', advertised_url: workerUrl, version: '0.1.0', schema_digest: COORDINATOR_SCHEMA_DIGEST,
    transport_mode: 'authenticated_remote', secret_ref: 'env:COORDINATOR_TOKEN', profiles: ['existing'], labels: [] };
  centralRegistry.registerNode(registration);
  const central = express();
  central.use(express.json());
  central.use('/api', authMiddleware);
  central.use('/api', projectRoutes({ node: { role: 'central', central_url: null }, registry: centralRegistry, localNodeId: 'central',
    listProfiles: async () => { centralProfileCalls += 1; return []; }, addProfile: async () => { throw new Error('Central must never create this profile'); } }));
  const centralServer = createServer(central);
  const centralUrl = await listen(centralServer);
  try {
    const remoteHeaders = { 'X-Forwarded-For': '198.51.100.1' };
    assert.equal((await post(`${workerUrl}/api/worker/projects/import`, { gitUrl, nodeId: 'worker' }, remoteHeaders)).status, 401);
    assert.equal((await post(`${workerUrl}/api/projects/import`, { gitUrl })).status, 409);
    assert.equal((await post(`${centralUrl}/api/worker/projects/import`, { gitUrl, nodeId: 'worker' })).status, 409);
    assert.equal((await post(`${centralUrl}/api/projects/import`, { gitUrl, nodeId: 'worker' }, remoteHeaders)).status, 401);
    redirectHealth = true;
    assert.equal((await post(`${centralUrl}/api/projects/import`, { gitUrl, nodeId: 'worker' })).status, 502);
    assert.equal(unexpectedRequests, 0, 'worker requests never follow redirects');
    redirectHealth = false;
    healthNodeId = 'wrong-worker';
    assert.equal((await post(`${centralUrl}/api/projects/import`, { gitUrl, nodeId: 'worker' })).status, 502);
    assert.equal(workerImports, 0, 'identity preflight fails before clone/profile mutation');
    assert.equal(existsSync(environment.GAH_PROJECT_CATALOG_PATH), false);
    healthNodeId = 'worker';
    const response = await post(`${centralUrl}/api/projects/import`, { gitUrl, nodeId: 'worker' });
    assert.equal(response.status, 201, await response.clone().text());
    const result = await response.json() as ProjectImportResult;
    assert.equal(result.project.node_id, 'worker');
    assert.equal(result.project.chat_profile, 'gah-node:worker:owner-repository');
    assert.equal(result.checkoutStatus, 'cloned');
    assert.ok(existsSync(join(result.checkoutPath, '.git')));
    assert.equal(centralProfileCalls, 0, 'remote import never reads or creates local profiles');
    assert.deepEqual(centralRegistry.getNode('worker')?.profiles, ['existing', 'owner-repository']);
    assert.equal(centralRegistry.getNode('worker')?.secret_ref, registration.secret_ref);
    const catalog = JSON.parse(readFileSync(environment.GAH_PROJECT_CATALOG_PATH, 'utf8'));
    assert.equal(catalog.projects[0].node_id, 'worker');
    await close(workerServer);
    const offline = await post(`${centralUrl}/api/projects/import`, { gitUrl: 'https://github.com/owner/other', nodeId: 'worker' });
    assert.equal(offline.status, 502);
    assert.match(await offline.text(), /offline or unreachable/);
    assert.equal(workerProfiles.length, 1);
    const listed = await (await fetch(`${centralUrl}/api/projects`)).json() as ProjectImportResult['project'][];
    assert.equal(listed[0].node_id, 'worker', 'offline projects remain visible');
    assert.equal((await fetch(`${centralUrl}/api/projects/owner-repository?nodeId=worker`, { method: 'DELETE' })).status, 200);
    assert.ok(existsSync(result.checkoutPath), 'catalog removal never deletes worker checkout');
  } finally {
    if (workerServer.listening) await close(workerServer);
    await close(centralServer);
    for (const [key, value] of Object.entries(saved)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; }
    rmSync(root, { recursive: true, force: true });
  }
});
