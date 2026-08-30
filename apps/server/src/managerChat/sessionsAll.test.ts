import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import type { ChatSessionSummary, ProfileSummary } from '@git-agent-harness/contracts';
import { resetCachedCoordinatorIdentity } from '../coordinatorIdentity.js';
import { createServer } from '../server.js';
import { setChatSessionStoreOptions, listAllChatSessions } from './chatSessions.js';

const NOW = 1_800_000_000_000;

function sessionSummary(profile: string, id: string, lastActiveAt: number, outcome: ChatSessionSummary['outcome'] = 'live'): ChatSessionSummary {
  const archived = outcome !== 'live';
  return {
    id,
    profile,
    worktreePath: null,
    branch: `gah/chat/x-${id}`,
    backend: 'codex',
    model: null,
    reasoningEffort: null,
    title: `${profile} ${id}`,
    createdAt: lastActiveAt - 1_000,
    lastActiveAt,
    archivedAt: archived ? lastActiveAt : null,
    outcome,
    settledAt: outcome === 'settled' ? lastActiveAt : null,
    settledReason: outcome === 'settled' ? 'merged' : null
  };
}

function writeSessionIndex(stateDir: string, profile: string, sessions: ChatSessionSummary[]): void {
  const profileDir = join(stateDir, `project-${encodeURIComponent(profile)}`);
  mkdirSync(profileDir, { recursive: true });
  writeFileSync(join(profileDir, 'sessions.json'), JSON.stringify({ sessions }));
}

test('listAllChatSessions scans every project dir, sorts within and across projects', () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-sessions-all-'));
  try {
    setChatSessionStoreOptions({ stateDir });
    writeSessionIndex(stateDir, 'zeta', [
      sessionSummary('zeta', 'older', NOW - 5_000),
      sessionSummary('zeta', 'newer', NOW),
      sessionSummary('zeta', 'archived', NOW - 1_000, 'settled')
    ]);
    writeSessionIndex(stateDir, 'alpha', [sessionSummary('alpha', 'only', NOW - 2_000)]);
    // A project dir with no sessions file (or an unreadable one) is skipped.
    mkdirSync(join(stateDir, 'project-empty'), { recursive: true });

    const groups = listAllChatSessions();
    assert.deepEqual(groups.map((group) => group.profile), ['alpha', 'zeta']);
    assert.deepEqual(groups[0].sessions.map((session) => session.id), ['only']);
    assert.deepEqual(groups[1].sessions.map((session) => session.id), ['newer', 'archived', 'older']);
    assert.equal(groups[1].sessions[1].outcome, 'settled');
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
    rmSync(stateDir, { recursive: true, force: true });
  }
});

test('listAllChatSessions returns nothing when the state dir is missing', () => {
  setChatSessionStoreOptions({ stateDir: join(tmpdir(), `gah-sessions-all-missing-${Date.now()}`) });
  try {
    assert.deepEqual(listAllChatSessions(), []);
  } finally {
    setChatSessionStoreOptions({ stateDir: undefined });
  }
});

test('GET /api/manager-chat/sessions/all groups sessions by project with display names', async () => {
  const stateDir = mkdtempSync(join(tmpdir(), 'gah-sessions-all-route-'));
  const profiles: ProfileSummary[] = [{
    name: 'alpha',
    display_name: 'Alpha Project',
    provider: 'github',
    repo: 'org/alpha',
    local_path: '/tmp/alpha',
    web_url: null,
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    validation_timeout_seconds: 300,
    chat_session_idle_days: 14,
    manager_wake_autonomy: 'off',
    delivery_mode: 'pr',
    repo_id: 'alpha',
    worktree_base: '/tmp/worktrees'
  }];
  writeSessionIndex(stateDir, 'alpha', [sessionSummary('alpha', 'a1', NOW)]);
  writeSessionIndex(stateDir, 'unlisted', [sessionSummary('unlisted', 'u1', NOW)]);
  setChatSessionStoreOptions({ stateDir });

  resetCachedCoordinatorIdentity();
  const identityDir = mkdtempSync(join(tmpdir(), 'gah-sessions-all-identity-'));
  const savedIdentityPath = process.env.GAH_COORDINATOR_IDENTITY_PATH;
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(identityDir, 'coordinator-identity.json');
  resetCachedCoordinatorIdentity();
  const app = createServer({ runProfileList: () => Promise.resolve(profiles) });
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, resolve));
  const { port } = server.address() as AddressInfo;
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/manager-chat/sessions/all`);
    assert.equal(response.status, 200);
    const body = await response.json() as {
      projects: { profile: string; profileName: string; sessions: { id: string }[] }[];
    };
    assert.deepEqual(body.projects.map((group) => group.profile), ['alpha', 'unlisted']);
    assert.equal(body.projects[0].profileName, 'Alpha Project');
    // A project missing from the profile list falls back to the raw id.
    assert.equal(body.projects[1].profileName, 'unlisted');
    assert.deepEqual(body.projects[0].sessions.map((session) => session.id), ['a1']);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    setChatSessionStoreOptions({ stateDir: undefined });
    if (savedIdentityPath === undefined) delete process.env.GAH_COORDINATOR_IDENTITY_PATH;
    else process.env.GAH_COORDINATOR_IDENTITY_PATH = savedIdentityPath;
    resetCachedCoordinatorIdentity();
    rmSync(stateDir, { recursive: true, force: true });
    rmSync(identityDir, { recursive: true, force: true });
  }
});
