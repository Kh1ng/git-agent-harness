import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer as createHttpServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { createServer } from './server.js';
import { getCoordinatorIdentity, resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import { addRemoteProject } from './projectCatalog.js';
import { RegistryService } from './registryService.js';

test('issue and PR starts use a healthy equivalent checkout instead of the offline catalog owner', async t => {
  const root = mkdtempSync(join(tmpdir(), 'gah-source-routing-'));
  const savedEnv = { ...process.env };
  const fixtures = new URL('../tests/fixtures/', import.meta.url).pathname;
  Object.assign(process.env, {
    GAH_BINARY: join(fixtures, 'gah/gah'), GAH_FIXTURE_PROFILE_LIST: join(root, 'profiles.json'),
    GAH_COORDINATOR_IDENTITY_PATH: join(root, 'identity.json'), GAH_PROJECT_CATALOG_PATH: join(root, 'projects.json'),
    GAH_CHAT_STATE_DIR: join(root, 'chat'), GAH_GATEWAY_SETTINGS_PATH: join(root, 'gateway.json'),
    GAH_MANAGER_CHAT_SETTINGS_PATH: join(root, 'manager.json'),
    GAH_FAKE_GH_FIXTURE: join(fixtures, 'gh/data'), GAH_FAKE_GH_STATE: join(root, 'gh-state'),
    PATH: `${join(fixtures, 'gh')}:${process.env.PATH}`
  });
  resetCachedCoordinatorIdentity();
  const central = getCoordinatorIdentity();
  const checkout = join(root, 'checkout');
  execFileSync('git', ['init', '--quiet', '--initial-branch=main', checkout]);
  for (const [key, value] of [['user.email', 'test@gah'], ['user.name', 'Test']]) execFileSync('git', ['config', key, value], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'initial'], { cwd: checkout });
  const fixture: ProfileSummary = JSON.parse(readFileSync(join(fixtures, 'gah/responses/profile-list.json'), 'utf8'))[0];
  const profile = { ...fixture, name: 'source-project', local_path: checkout, worktree_base: join(root, 'worktrees') };
  writeFileSync(process.env.GAH_FIXTURE_PROFILE_LIST!, JSON.stringify([profile]));
  const registry = new RegistryService(join(root, 'registry.json'));
  registry.registerNode({ node_id: 'offline', display_name: 'Offline owner', advertised_url: 'http://127.0.0.1:1', version: central.version, schema_digest: central.schema_digest, transport_mode: 'loopback', secret_ref: 'env:COORDINATOR_TOKEN', profiles: [profile.name], last_observed_at: new Date().toISOString(), last_observed_state: 'unreachable' });
  const project = addRemoteProject('offline', profile);
  const server = createHttpServer(createServer({ registryService: registry }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
    process.env = savedEnv;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  });
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const base = `http://127.0.0.1:${address.port}`;
  for (const [source, number] of [['issue', 42], ['pr', 12]] as const) {
    const pinned = await fetch(`${base}/api/manager-chat/${source}s/start`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile: project.chat_profile, [`${source}Number`]: number, backend: 'codex', nodeId: 'offline' })
    });
    assert.equal(pinned.status, 502, 'an explicit offline pin is never silently replaced');
    if (source === 'issue') assert.equal(existsSync(process.env.GAH_FAKE_GH_STATE!), false, 'readiness failure never claims the issue');
    const response = await fetch(`${base}/api/manager-chat/${source}s/start`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile: project.chat_profile, [`${source}Number`]: number, backend: 'codex', ...(source === 'issue' ? { nodeId: central.node_id } : {}) })
    });
    const body = await response.json() as { session: ChatSessionSummary; message?: string };
    assert.equal(response.status, 201, body.message);
    assert.equal(body.session.nodeId, central.node_id);
    assert.equal(body.session.profile, project.chat_profile);
    assert.equal(body.session[source === 'issue' ? 'issueNumber' : 'prNumber'], number);
    assert.equal(body.session.worktreePath === null, source === 'pr');
    const unpinned = await fetch(`${base}/api/manager-chat/${source}s/start`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ profile: project.chat_profile, [`${source}Number`]: number, backend: 'codex' })
    });
    assert.equal(unpinned.status, 201, 'legacy callers without a node recover from an offline owner');
    assert.equal((await unpinned.json() as { session: ChatSessionSummary }).session.id, body.session.id, 'idempotent source opening stays intact');
  }
  assert.ok(existsSync(process.env.GAH_FAKE_GH_STATE!), 'issue is claimed after successful routing');
});
