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
import { createChatSession, startChatFromIssue, startChatFromPr, sendManagerChatMessage, archiveChatSession, getSessionView } from './managerChat/ManagerChatManager.js';
import { getSession } from './managerChat/chatSessions.js';
import type { ManagerAdapter } from './managerChat/registry.js';
import { reclaimChatSessions } from './managerChat/chatMaintenance.js';
import { putSkill, setSkillBindings } from './skillBank.js';

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
    GAH_FAKE_GH_FIXTURE: new URL('../tests/fixtures/gh/data', import.meta.url).pathname,
    GAH_FAKE_GH_STATE: join(root, 'gh-state'),
    PATH: `${new URL('../tests/fixtures/gh', import.meta.url).pathname}:${process.env.PATH}`,
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
  const unavailableForHandoff = new Set<string>();
  const seed = join(root, 'seed');
  const remote = join(root, 'remote.git');
  mkdirSync(seed);
  for (const args of [['init', '--quiet', '--initial-branch=main'], ['config', 'user.email', 'test@gah'], ['config', 'user.name', 'Test']]) execFileSync('git', args, { cwd: seed });
  writeFileSync(join(seed, 'README.md'), '# worker');
  execFileSync('git', ['add', '.'], { cwd: seed });
  execFileSync('git', ['commit', '--quiet', '-m', 'initial'], { cwd: seed });
  execFileSync('git', ['clone', '--quiet', '--bare', seed, remote]);
  for (const nodeId of ['one', 'two']) {
    const checkout = join(root, nodeId);
    execFileSync('git', ['clone', '--quiet', remote, checkout]);
    execFileSync('git', ['config', 'user.email', 'test@gah'], { cwd: checkout });
    execFileSync('git', ['config', 'user.name', 'Test'], { cwd: checkout });
    const profile = { ...fixture, name: 'worker-only', local_path: checkout, worktree_base: join(root, `${nodeId}-worktrees`) };
    profiles.push(profile);
    const adapter: ManagerAdapter = {
      id: 'claude', displayName: 'Test agent', implemented: true,
      async runTurn(_key, input) {
        assert.ok(input.cwd?.startsWith(profile.worktree_base));
        assert.match(input.prompt, /central memory fact/);
        if (seen.length === 0) {
          assert.match(input.prompt, /central skill instruction/);
          assert.doesNotMatch(input.prompt, /chat-only skill instruction/);
        } else {
          assert.match(input.prompt, /chat-only skill instruction/);
          assert.doesNotMatch(input.prompt, /central skill instruction/);
        }
        if (seen.length) assert.ok(input.history.some(turn => turn.text === 'reply from one'));
        if (nodeId === 'two' && seen.length === 1) assert.equal(readFileSync(join(input.cwd!, 'unfinished.txt'), 'utf8'), 'work on one');
        if (nodeId === 'one' && seen.length === 2) assert.equal(readFileSync(join(input.cwd!, 'unfinished.txt'), 'utf8'), 'work on two');
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
    app.use(express.json(), (req, res, next) => {
      if (unavailableForHandoff.has(nodeId) && req.body?.action === 'handoff') return void res.status(503).json({ error: 'offline' });
      next();
    }, rateLimit({ windowMs: 60_000, limit: 200, validate: false }), authMiddleware, workerRouteGuard(node));
    app.get('/api/status', (_req, res) => res.json({ node_id: nodeId, generated_at: new Date().toISOString(), profile: { profile: profile.name }, backend_configured: { claude: true }, backend_instances: [], availability: [] }));
    app.use('/api/worker-chat', createWorkerChatRouter({ node, nodeId, profiles: async () => [profile], adapter: () => adapter, sessions: { stateDir: join(root, `${nodeId}-state`) } }));
    const url = await listen(createServer(app));
    registry.registerNode({ node_id: nodeId, display_name: nodeId, advertised_url: url, version: central.version, schema_digest: central.schema_digest, transport_mode: 'loopback', secret_ref: 'env:COORDINATOR_TOKEN', profiles: [profile.name], last_observed_at: new Date().toISOString(), last_observed_state: 'healthy' });
    const denied = await fetch(`${url}/api/worker-chat`, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Forwarded-For': '192.0.2.10' }, body: '{}' });
    assert.equal(denied.status, 401);
  }
  const project = addRemoteProject('one', profiles[0]);
  putSkill({ id: 'central', version: '1', displayName: 'Central', description: '', content: 'central skill instruction', backends: ['claude'], source: 'test', createdAt: 1, updatedAt: 1 });
  putSkill({ id: 'chat-only', version: '1', displayName: 'Chat only', description: '', content: 'chat-only skill instruction', backends: ['claude'], source: 'test', createdAt: 1, updatedAt: 1 });
  setSkillBindings(project.chat_profile, 'claude', ['central']);
  assert.match((await chatNodes(project.chat_profile, 'claude')).find(node => node.nodeId === 'one')!.reason!, /readiness is unknown/);
  await registry.getNodeObservations();
  const nodes = await chatNodes(project.chat_profile, 'claude');
  assert.equal(nodes.find(node => node.role === 'central')?.eligible, false);
  const session = await createChatSession(project.chat_profile, 'claude');
  const first = await sendManagerChatMessage(project.chat_profile, 'first task', 'first', session.id);
  setSkillBindings(project.chat_profile, 'claude', ['chat-only'], { sessionId: session.id });
  const second = await sendManagerChatMessage(project.chat_profile, 'continue here', 'second', session.id, undefined, 'two');
  const third = await sendManagerChatMessage(project.chat_profile, 'move back', 'third', session.id, undefined, 'one');
  unavailableForHandoff.add('one');
  const fourth = await sendManagerChatMessage(project.chat_profile, 'continue despite offline source', 'fourth', session.id, undefined, 'two');
  assert.deepEqual(seen, ['one', 'two', 'one', 'two']);
  assert.equal(first.turn.nodeId, 'one');
  assert.equal(second.turn.nodeId, 'two');
  assert.equal(third.turn.nodeId, 'one');
  assert.equal(fourth.turn.nodeId, 'two');
  assert.equal(recalls, 4);
  const sessionView = JSON.stringify(getSessionView(project.chat_profile, session.id));
  assert.match(sessionView, /reply from one/);
  assert.match(sessionView, /\[handoff: one → two\] Workspace carried in commit [0-9a-f]+/);
  assert.match(sessionView, /\[handoff: two → one\] Workspace carried in commit [0-9a-f]+/);
  assert.match(sessionView, /\[handoff: one → two\] Changes on one were not carried/);
  assert.match(sessionView, /\[skills · claude · session\] chat-only@1/);
  const saved = getSession(project.chat_profile, session.id)!;
  assert.equal(saved.createdAt, session.createdAt, 'worker switches preserve the central conversation creation date');
  assert.deepEqual(saved.workspaceNodes?.sort(), ['one', 'two']);
  assert.notEqual(saved.workspaces?.one.worktreePath, saved.workspaces?.two.worktreePath);
  assert.ok(existsSync(join(saved.workspaces!.one.worktreePath!, 'unfinished.txt')));
  assert.equal(readFileSync(join(saved.workspaces!.two.worktreePath!, 'unfinished.txt'), 'utf8'), 'work on two');
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
  // Source chats use the same worker RPC and preserve an explicitly chosen
  // account, including PR's worktree-less creation contract.
  for (const [start, number, worktree] of [[startChatFromIssue, 42, true], [startChatFromPr, 12, false]] as const) {
    const created = (await start(project.chat_profile, number, 'claude', null, 'two', 'claude-2')).session;
    assert.equal(created.nodeId, 'two');
    assert.equal(created.backendInstance, 'claude-2');
    assert.equal(created.worktreePath !== null, worktree);
    assert.equal(getSession('worker-only', created.id, { stateDir: join(root, 'two-state') })?.backendInstance, 'claude-2');
  }
});
