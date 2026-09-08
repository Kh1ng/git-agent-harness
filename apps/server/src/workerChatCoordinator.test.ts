import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer, type Server } from 'node:http';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import express from 'express';
import rateLimit from 'express-rate-limit';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { createWorkerChatRouter } from './workerChat.js';
import { authMiddleware } from './authMiddleware.js';
import { workerRouteGuard } from './nodeRole.js';
import { RegistryService } from './registryService.js';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { addRemoteProject } from './projectCatalog.js';
import { configureChatRouting, chatNodes } from './chatRouting.js';
import { createChatSession, sendManagerChatMessage, archiveChatSession, getSessionView } from './managerChat/ManagerChatManager.js';
import { getSession } from './managerChat/chatSessions.js';
import type { ManagerAdapter } from './managerChat/registry.js';
import { reclaimChatSessions } from './managerChat/chatMaintenance.js';
import { putSkill, setProfileSkillBindings } from './skillBank.js';

test('central keeps history and skills while authenticated turns move between worker checkouts', { timeout: 20_000 }, async t => {
  const root = mkdtempSync(join(tmpdir(), 'gah-central-worker-'));
  const savedEnv = { ...process.env };
  const servers: Server[] = [];
  t.after(async () => {
    for (const server of servers) {
      server.closeAllConnections();
      await new Promise<void>(resolve => server.close(() => resolve()));
    }
    process.env = savedEnv;
    rmSync(root, { recursive: true, force: true });
  });
  Object.assign(process.env, {
    GAH_BINARY: new URL('../tests/fixtures/gah/gah', import.meta.url).pathname,
    GAH_FIXTURE_PROFILE_LIST: join(root, 'profiles.json'),
    GAH_COORDINATOR_IDENTITY_PATH: join(root, 'identity.json'),
    GAH_PROJECT_CATALOG_PATH: join(root, 'projects.json'),
    GAH_CHAT_STATE_DIR: join(root, 'central-chat'),
    GAH_GATEWAY_SETTINGS_PATH: join(root, 'gateway.json'),
    GAH_MANAGER_CHAT_SETTINGS_PATH: join(root, 'manager.json'),
    GAH_SKILL_BANK_PATH: join(root, 'skills.json'),
    COORDINATOR_TOKEN: 'isolated-worker-test',
    GAH_ALLOW_INSECURE_HTTP: '1'
  });
  writeFileSync(process.env.GAH_FIXTURE_PROFILE_LIST!, '[]');
  let recalls = 0;
  const gateway = createServer((req, res) => {
    req.resume();
    if (req.url === '/recall') { recalls++; res.end(JSON.stringify({ context: 'central memory fact', memory_count: 1, code: 0, message: 'ok' })); }
    else res.end(JSON.stringify({ l0_recorded: 1, scheduler_notified: false, flushed: true }));
  });
  async function listen(server: Server): Promise<string> {
    servers.push(server);
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    assert.ok(address && typeof address !== 'string');
    return `http://127.0.0.1:${address.port}`;
  }
  process.env.TDAI_GATEWAY_URL = await listen(gateway);
  const central = getCoordinatorIdentity();
  const registry = new RegistryService(join(root, 'registry.json'));
  configureChatRouting(registry);
  const fixture: ProfileSummary = JSON.parse(readFileSync(new URL('../tests/fixtures/gah/responses/profile-list.json', import.meta.url), 'utf8'))[0];
  const profiles: ProfileSummary[] = [];
  const seen: string[] = [];
  for (const nodeId of ['one', 'two']) {
    const checkout = join(root, nodeId);
    mkdirSync(checkout);
    for (const args of [['init', '--quiet', '--initial-branch=main'], ['config', 'user.email', 'test@gah'], ['config', 'user.name', 'Test']]) execFileSync('git', args, { cwd: checkout });
    writeFileSync(join(checkout, 'README.md'), '# worker');
    execFileSync('git', ['add', '.'], { cwd: checkout });
    execFileSync('git', ['commit', '--quiet', '-m', 'initial'], { cwd: checkout });
    const profile = { ...fixture, name: 'worker-only', local_path: checkout, worktree_base: join(root, `${nodeId}-worktrees`) };
    profiles.push(profile);
    const adapter: ManagerAdapter = {
      id: 'claude', displayName: 'Test agent', implemented: true,
      async runTurn(_key, input) {
        assert.ok(input.cwd?.startsWith(profile.worktree_base));
        assert.match(input.prompt, /central memory fact/);
        assert.match(input.prompt, /central skill instruction/);
        if (seen.length) assert.ok(input.history.some(turn => turn.text === 'reply from one'));
        seen.push(nodeId);
        writeFileSync(join(input.cwd!, 'unfinished.txt'), `work on ${nodeId}`);
        return { reply: `reply from ${nodeId}`, model: null, usage: null };
      },
      listCommands: async () => [],
      listModels: async () => ({ models: [], currentModelId: null, reasoningEfforts: [], currentReasoningEffortId: null, contextUsage: null }),
      setModel: async () => {}, setReasoningEffort: async () => {},
      steerTurn: async () => ({ outcome: 'injected' }), cancelTurn: async () => {}
    };
    const node = { role: 'worker' as const, central_url: 'https://central.test' };
    const app = express();
    app.use(express.json(), rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware, workerRouteGuard(node));
    app.get('/api/status', (_req, res) => res.json({ node_id: nodeId, generated_at: new Date().toISOString(), profile: { profile: profile.name }, backend_configured: { claude: true }, backend_instances: [], availability: [] }));
    app.use('/api/worker-chat', createWorkerChatRouter({ node, nodeId, profiles: async () => [profile], adapter: () => adapter, sessions: { stateDir: join(root, `${nodeId}-state`) } }));
    const url = await listen(createServer(app));
    registry.registerNode({ node_id: nodeId, display_name: nodeId, advertised_url: url, version: central.version, schema_digest: central.schema_digest, transport_mode: 'loopback', secret_ref: 'env:COORDINATOR_TOKEN', profiles: [profile.name], last_observed_at: new Date().toISOString(), last_observed_state: 'healthy' });
    const denied = await fetch(`${url}/api/worker-chat`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Forwarded-For': '192.0.2.10' }, body: '{}' });
    assert.equal(denied.status, 401);
  }
  const project = addRemoteProject('one', profiles[0]);
  putSkill({ id: 'central', version: '1', displayName: 'Central', description: '', content: 'central skill instruction', backends: ['claude'], source: 'test', createdAt: 1, updatedAt: 1 });
  setProfileSkillBindings(project.chat_profile, 'claude', ['central']);
  assert.match((await chatNodes(project.chat_profile, 'claude')).find(node => node.nodeId === 'one')!.reason!, /readiness is unknown/);
  await registry.getNodeObservations();
  const nodes = await chatNodes(project.chat_profile, 'claude');
  assert.equal(nodes.find(node => node.role === 'central')?.eligible, false);
  const session = await createChatSession(project.chat_profile, 'claude');
  const first = await sendManagerChatMessage(project.chat_profile, 'first task', 'first', session.id);
  const second = await sendManagerChatMessage(project.chat_profile, 'continue here', 'second', session.id, undefined, 'two');
  assert.deepEqual(seen, ['one', 'two']);
  assert.equal(first.turn.nodeId, 'one');
  assert.equal(second.turn.nodeId, 'two');
  assert.equal(recalls, 2);
  assert.match(JSON.stringify(getSessionView(project.chat_profile, session.id)), /reply from one/);
  const saved = getSession(project.chat_profile, session.id)!;
  assert.equal(saved.createdAt, session.createdAt, 'worker switches preserve the central conversation creation date');
  assert.deepEqual(saved.workspaceNodes?.sort(), ['one', 'two']);
  assert.notEqual(saved.workspaces?.one.worktreePath, saved.workspaces?.two.worktreePath);
  assert.ok(existsSync(join(saved.workspaces!.one.worktreePath!, 'unfinished.txt')));
  const storage = await reclaimChatSessions({ profile: project.chat_profile, dryRun: true });
  assert.equal(storage.profiles[0].worktreeBytes, null);
  assert.deepEqual(storage.candidates, []);
  const archiving = archiveChatSession(project.chat_profile, session.id);
  await assert.rejects(sendManagerChatMessage(project.chat_profile, 'during archive', 'blocked', session.id), /busy|archived/);
  const archived = await archiving;
  assert.ok(archived.archivedAt);
  for (const nodeId of ['one', 'two']) {
    assert.ok(getSession('worker-only', session.id, { stateDir: join(root, `${nodeId}-state`) })?.archivedAt);
    assert.equal(existsSync(saved.workspaces![nodeId].worktreePath!), false);
  }
});
