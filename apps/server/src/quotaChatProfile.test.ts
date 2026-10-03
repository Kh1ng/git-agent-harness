import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer as createHttpServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { createServer } from './server.js';
import { getCoordinatorIdentity, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { addRemoteProject } from './projectCatalog.js';
import { RegistryService } from './registryService.js';

test('quota reads after an issue chat resolve its catalog identity for native and worker snapshots', async t => {
  const root = mkdtempSync(join(tmpdir(), 'gah-quota-chat-profile-'));
  const savedEnv = { ...process.env };
  const fixtures = new URL('../tests/fixtures/', import.meta.url).pathname;
  const binary = join(root, 'gah.mjs');
  const quotaLog = join(root, 'quota-argv.json');
  writeFileSync(binary, `#!/usr/bin/env node
import { writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
const args = process.argv.slice(2);
if (args[0] === 'quota') {
  writeFileSync(${JSON.stringify(quotaLog)}, JSON.stringify(args));
  if (args[args.indexOf('--profile') + 1] !== 'gah') process.exit(2);
}
const result = spawnSync(${JSON.stringify(join(fixtures, 'gah/gah'))}, args, { stdio: 'inherit' });
process.exit(result.status ?? 1);
`, { mode: 0o755 });
  Object.assign(process.env, {
    GAH_BINARY: binary, GAH_FIXTURE_PROFILE_LIST: join(root, 'profiles.json'),
    GAH_COORDINATOR_IDENTITY_PATH: join(root, 'identity.json'), GAH_PROJECT_CATALOG_PATH: join(root, 'projects.json'),
    GAH_CHAT_STATE_DIR: join(root, 'chat'), GAH_GATEWAY_SETTINGS_PATH: join(root, 'gateway.json'),
    GAH_MANAGER_CHAT_SETTINGS_PATH: join(root, 'manager.json'),
    GAH_FAKE_GH_FIXTURE: join(fixtures, 'gh/data'), GAH_FAKE_GH_STATE: join(root, 'gh-state'),
    PATH: `${join(fixtures, 'gh')}:${process.env.PATH}`
  });
  resetCachedCoordinatorIdentity();
  t.after(() => {
    process.env = savedEnv;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  });
  const central = getCoordinatorIdentity();
  const checkout = join(root, 'checkout');
  execFileSync('git', ['init', '--quiet', '--initial-branch=main', checkout]);
  for (const [key, value] of [['user.email', 'test@gah'], ['user.name', 'Test']]) execFileSync('git', ['config', key, value], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'initial'], { cwd: checkout });
  const fixture: ProfileSummary = JSON.parse(readFileSync(join(fixtures, 'gah/responses/profile-list.json'), 'utf8'))[0];
  const profile = { ...fixture, name: 'gah', local_path: checkout, worktree_base: join(root, 'worktrees') };
  writeFileSync(process.env.GAH_FIXTURE_PROFILE_LIST!, JSON.stringify([profile]));
  const registry = new RegistryService(join(root, 'registry.json'));
  registry.registerNode({ node_id: 'offline', display_name: 'Offline owner', advertised_url: 'http://127.0.0.1:1', version: central.version, schema_digest: central.schema_digest, transport_mode: 'loopback', secret_ref: 'env:COORDINATOR_TOKEN', profiles: ['gah'], last_observed_state: 'unreachable' });
  const project = addRemoteProject('offline', profile);
  const server = createHttpServer(createServer({ registryService: registry }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); registry.stopLivenessScheduler(); });
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const base = `http://127.0.0.1:${address.port}`;
  const started = await fetch(`${base}/api/manager-chat/issues/start`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ profile: project.chat_profile, issueNumber: 42, backend: 'codex', nodeId: central.node_id })
  });
  assert.equal(started.status, 201);
  const chatProfile = (await started.json() as { session: ChatSessionSummary }).session.profile;
  assert.equal(chatProfile, project.chat_profile);

  const workerRequests: string[] = [];
  const workerQuota = JSON.parse(readFileSync(join(fixtures, 'gah/responses/quota.json'), 'utf8'));
  workerQuota.profile.profile = 'gah';
  const worker = createHttpServer((req, res) => {
    workerRequests.push(req.url ?? '');
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify(workerQuota));
  });
  await new Promise<void>(resolve => worker.listen(0, '127.0.0.1', resolve));
  t.after(() => { worker.closeAllConnections(); worker.close(); });
  const workerAddress = worker.address();
  assert.ok(workerAddress && typeof workerAddress !== 'string');
  registry.registerNode({ node_id: 'worker', display_name: 'Worker', advertised_url: `http://127.0.0.1:${workerAddress.port}`, version: central.version, schema_digest: central.schema_digest, transport_mode: 'loopback', secret_ref: 'env:UNUSED', profiles: ['gah'] });

  const statuses: number[] = [];
  for (const route of ['quota', 'registry/quota']) {
    const response = await fetch(`${base}/api/${route}?profile=${encodeURIComponent(chatProfile)}&since=7d`);
    statuses.push(response.status);
    if (route === 'registry/quota' && response.ok) {
      const body = await response.json() as { nodes: { nodeId: string; state: string }[] };
      assert.equal(body.nodes.find(node => node.nodeId === 'worker')?.state, 'available');
    }
  }
  assert.deepEqual(statuses, [200, 200], 'the active issue-chat identity must work on both quota APIs');
  const argv: string[] = JSON.parse(readFileSync(quotaLog, 'utf8'));
  assert.equal(argv[argv.indexOf('--profile') + 1], 'gah');
  assert.deepEqual(workerRequests, ['/api/quota?profile=gah&since=7d']);

  for (const route of ['quota', 'registry/quota']) {
    for (const alias of ['gah-node:unknown:gah', 'gah-node:offline:unregistered', 'gah-node:offline:%ZZ']) {
      assert.equal((await fetch(`${base}/api/${route}?profile=${encodeURIComponent(alias)}`)).status, 404, 'only registered catalog identities can resolve');
    }
  }
  assert.equal((await fetch(`${base}/api/registry/quota?profile=not%3Aa%3Aprofile`)).status, 400);
  assert.equal((await fetch(`${base}/api/registry/quota?profile=${encodeURIComponent(chatProfile)}&since=bad`)).status, 400);
  assert.deepEqual(workerRequests, ['/api/quota?profile=gah&since=7d'], 'rejected identities must never reach a worker');
});
