import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer, initializeSkillBank } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import type { ConfigProfileSummary, DoctorSnapshot, ProfileSummary, SettingsConfigProfileSummary } from '@git-agent-harness/contracts';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { archiveSession, createSession } from './managerChat/chatSessions.js';

const gitFixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures');

function profilePayload(profile: string): ConfigProfileSummary {
  return {
    profile,
    delivery_mode: 'pr',
    merge_policy: 'auto',
    max_fix_attempts_per_mr: 2,
    max_implementation_failures_per_ticket: 8,
    max_review_cycles_per_ticket: 3,
    max_paid_reviews_per_ticket: 3,
    backend_instances: [],
    pm_candidates: [],
    improve_candidates: [],
    review_candidates: [],
    task_routing_rules: [],
    routine_reviewer: null,
    escalatory_reviewers: [],
    context: {
      global: {
        enabled: true,
        soft_limit_tokens: 80_000,
        hard_limit_tokens: 150_000,
        compact_after_tool_calls: 20,
        fresh_context_on_review: true,
        fresh_context_on_fix: true,
        include_full_git_history: false,
        include_full_worker_transcript_in_review: false,
        recent_history_tokens: 20_000
      },
      profile_override: null,
      effective_by_backend: []
    },
    notifications: {
      configured: false,
      transport: null,
      manager_wake_autonomy: 'off',
      env_file: null,
      env_file_prod: null
    }
  };
}

async function withTestServer(
  runProfile: (profile: string) => Promise<ConfigProfileSummary>,
  testFn: (url: string) => Promise<void>,
  runDoctor?: (profile: string) => Promise<DoctorSnapshot>,
  coordinatorPort?: number,
  runProfileList?: () => Promise<ProfileSummary[]>
) {
  resetCachedCoordinatorIdentity();
  const tmpIdentityDir = mkdtempSync(join(tmpdir(), 'gah-test-identity-'));
  const savedIdentityPath = process.env.GAH_COORDINATOR_IDENTITY_PATH;
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(tmpIdentityDir, 'coordinator-identity.json');
  resetCachedCoordinatorIdentity();
  const app = createServer({
    runConfigShowProfile: runProfile,
    ...(runDoctor ? { runDoctor } : {}),
    ...(coordinatorPort !== undefined ? { coordinatorPort } : {}),
    ...(runProfileList ? { runProfileList } : {})
  });
  const server = http.createServer(app);

  await new Promise<void>((resolve) => {
    server.listen(0, resolve);
  });

  const { port } = server.address() as AddressInfo;

  try {
    await testFn(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
    if (savedIdentityPath !== undefined) {
      process.env.GAH_COORDINATOR_IDENTITY_PATH = savedIdentityPath;
    } else {
      delete process.env.GAH_COORDINATOR_IDENTITY_PATH;
    }
    resetCachedCoordinatorIdentity();
  }
}

test('GET /api/config/effective returns profile JSON on success', async () => {
  let calledProfile = '';

  await withTestServer(async (profile) => {
    calledProfile = profile;
    return profilePayload(profile);
  }, async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/config/effective?profile=repo`);
    const body = (await response.json()) as SettingsConfigProfileSummary;

    assert.equal(response.status, 200);
    assert.equal(body.profile, 'repo');
    assert.equal(calledProfile, 'repo');
  });
});

test('GET /api/config/effective omits environment values while preserving configured status', async () => {
  const canary = 'TDAI_GATEWAY_API_KEY=GAH_TEST_CANARY_1014_DO_NOT_SERIALIZE';

  await withTestServer(async (profile) => {
    const payload = profilePayload(profile);
    payload.notifications.env_file = canary;
    payload.notifications.env_file_prod = `${canary}_PROD`;
    return payload;
  }, async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/config/effective?profile=repo`);
    const serialized = await response.text();
    const body = JSON.parse(serialized) as SettingsConfigProfileSummary;

    assert.equal(response.status, 200);
    assert.equal(serialized.includes(canary), false);
    assert.deepEqual(body.notifications, {
      configured: false,
      transport: null,
      manager_wake_autonomy: 'off',
      env_file_configured: true,
      env_file_prod_configured: true
    });
  });
});

test('GET /api/doctor returns structured failed checks as data', async () => {
  let calledProfile = '';
  const snapshot: DoctorSnapshot = {
    schema_version: 1,
    generated_at: '2026-07-21T00:00:00Z',
    overall_status: 'fail',
    checks: [{ profile: 'repo', name: 'backend executable', status: 'fail', detail: 'codex missing' }]
  };

  await withTestServer(
    async (profile) => profilePayload(profile),
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/doctor?profile=repo`);
      const body = (await response.json()) as DoctorSnapshot;
      assert.equal(response.status, 200);
      assert.equal(body.overall_status, 'fail');
      assert.equal(body.checks[0]?.detail, 'codex missing');
      assert.equal(calledProfile, 'repo');
    },
    async (profile) => {
      calledProfile = profile;
      return snapshot;
    }
  );
});

test('GET /api/config/effective falls back to default profile when profile query is missing', async () => {
  let calledProfile = '';

  await withTestServer(
    async (profile) => {
      calledProfile = profile;
      return profilePayload(profile);
    },
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/config/effective`);
      const body = (await response.json()) as SettingsConfigProfileSummary;

      assert.equal(response.status, 200);
      assert.equal(body.profile, 'gah');
      assert.equal(calledProfile, 'gah');
    }
  );
});

test('GET /api/config/effective returns 502 on lookup failures', async () => {
  await withTestServer(
    async () => {
      throw new Error('unknown profile');
    },
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/config/effective?profile=missing`);
      const body = (await response.json()) as {
        error?: string;
        message?: string;
      };

      assert.equal(response.status, 502);
      assert.equal(body.error, 'Failed to load effective config');
      assert.equal(body.message, 'unknown profile');
    }
  );
});

test('GET /api/info advertises the configured coordinator port', async () => {
  await withTestServer(
    async (profile) => profilePayload(profile),
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/info`);
      const body = (await response.json()) as {
        identity?: {
          advertised_url?: string;
        };
      };

      assert.equal(response.status, 200);
      assert.equal(body.identity?.advertised_url, 'http://localhost:9123');
    },
    undefined,
    9123
  );
});

test('project routes expose only curated profiles', async () => {
  const savedCatalogPath = process.env.GAH_PROJECT_CATALOG_PATH;
  process.env.GAH_PROJECT_CATALOG_PATH = join(mkdtempSync(join(tmpdir(), 'gah-project-routes-')), 'projects.json');
  const profiles: ProfileSummary[] = [{
    name: 'repo',
    display_name: 'Repo',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: '/repos/repo',
    worktree_base: '/worktrees',
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }];

  try {
    await withTestServer(
      async (profile) => profilePayload(profile),
      async (baseUrl) => {
        assert.deepEqual(await (await fetch(`${baseUrl}/api/projects`)).json(), []);

        const addResponse = await fetch(`${baseUrl}/api/projects`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ profile: 'repo' })
        });
        assert.equal(addResponse.status, 201);
        assert.equal(((await addResponse.json()) as ProfileSummary).name, 'repo');
        assert.deepEqual(
          ((await (await fetch(`${baseUrl}/api/projects`)).json()) as ProfileSummary[]).map((profile) => profile.name),
          ['repo']
        );

        const invalidImport = await fetch(`${baseUrl}/api/projects/import`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ gitUrl: 'file:///tmp/repo' })
        });
        assert.equal(invalidImport.status, 400);
      },
      undefined,
      undefined,
      async () => profiles
    );
  } finally {
    if (savedCatalogPath === undefined) delete process.env.GAH_PROJECT_CATALOG_PATH;
    else process.env.GAH_PROJECT_CATALOG_PATH = savedCatalogPath;
  }
});


test('skill bank API stores versions, resolves newest, and refuses deletion of a bound skill', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-api-'));
  const savedBank = process.env.GAH_SKILL_BANK_PATH;
  process.env.GAH_SKILL_BANK_PATH = join(dir, 'skills.json');
  try {
    initializeSkillBank();
    await withTestServer(async () => profilePayload('alpha'), async (baseUrl) => {
      const initial = (await (await fetch(`${baseUrl}/api/skills`)).json()) as { skills: Array<{ id: string }> };
      assert.deepEqual(initial.skills.map((skill) => skill.id), ['gah-manager']);

      const create = await fetch(`${baseUrl}/api/skills`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: 'alpha', version: '1.0.0', displayName: 'Alpha', description: 'd', content: 'v1', backends: ['hermes'] })
      });
      assert.equal(create.status, 201);
      await fetch(`${baseUrl}/api/skills`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: 'alpha', version: '2.0.0', displayName: 'Alpha', description: 'd', content: 'v2', backends: ['hermes'] })
      });

      // Unversioned GET resolves to the newest; versioned to the named one.
      const newest = (await (await fetch(`${baseUrl}/api/skills/alpha`)).json()) as { version: string };
      assert.equal(newest.version, '2.0.0');
      const v1 = (await (await fetch(`${baseUrl}/api/skills/alpha?version=1.0.0`)).json()) as { content: string };
      assert.equal(v1.content, 'v1');
      assert.equal((await fetch(`${baseUrl}/api/skills/nope`)).status, 404);

      const inherited = (await (await fetch(`${baseUrl}/api/skills/bindings?profile=alpha&backend=hermes`)).json()) as {
        source: string;
        selectedIds: string[];
      };
      assert.equal(inherited.source, 'canonical');
      assert.deepEqual(inherited.selectedIds, ['gah-manager']);

      const override = await fetch(`${baseUrl}/api/skills/bindings`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ profile: 'alpha', backend: 'hermes', skillIds: ['alpha'] })
      });
      assert.equal(override.status, 200);
      const resolved = (await (await fetch(`${baseUrl}/api/skills/resolve?profile=alpha&backend=hermes`)).json()) as {
        source: string;
        skills: Array<{ id: string; version: string; content: string }>;
      };
      assert.equal(resolved.source, 'profile');
      assert.deepEqual(resolved.skills.map(({ id, version }) => `${id}@${version}`), ['alpha@2.0.0']);
      assert.equal(resolved.skills[0]?.content, 'v2');

      const inherit = await fetch(`${baseUrl}/api/skills/bindings?profile=alpha&backend=hermes`, { method: 'DELETE' });
      assert.equal(inherit.status, 200);
      assert.deepEqual(((await inherit.json()) as { selectedIds: string[] }).selectedIds, ['gah-manager']);

      // AC7: deleting a skill that is bound is refused, naming the bindings.
      const { addBinding } = await import('./skillBank.js');
      addBinding('alpha', 'hermes:gah');
      const refused = await fetch(`${baseUrl}/api/skills/alpha`, { method: 'DELETE' });
      assert.equal(refused.status, 409);
      const refusedBody = (await refused.json()) as { message: string };
      assert.match(refusedBody.message, /still bound to hermes:gah/);
    });
  } finally {
    if (savedBank === undefined) delete process.env.GAH_SKILL_BANK_PATH;
    else process.env.GAH_SKILL_BANK_PATH = savedBank;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('skill bank initialization fails on a malformed bank but allows missing bank and seed files', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-startup-'));
  const bankPath = join(dir, 'skills.json');
  const savedBank = process.env.GAH_SKILL_BANK_PATH;
  const savedDocs = process.env.GAH_SKILL_DOCS_PATH;
  process.env.GAH_SKILL_BANK_PATH = bankPath;
  process.env.GAH_SKILL_DOCS_PATH = join(dir, 'missing-gah-manager-skill.md');
  writeFileSync(bankPath, JSON.stringify({ skills: [{ id: 'alpha' }], bindings: {} }), 'utf8');

  try {
    assert.throws(
      () => initializeSkillBank(),
      (error: unknown) => error instanceof Error
        && error.message.includes(`Invalid skill bank at ${bankPath}`)
        && error.message.includes('skills[0].version')
    );

    process.env.GAH_SKILL_BANK_PATH = join(dir, 'missing-skills.json');
    assert.doesNotThrow(() => initializeSkillBank());
  } finally {
    if (savedBank === undefined) delete process.env.GAH_SKILL_BANK_PATH;
    else process.env.GAH_SKILL_BANK_PATH = savedBank;
    if (savedDocs === undefined) delete process.env.GAH_SKILL_DOCS_PATH;
    else process.env.GAH_SKILL_DOCS_PATH = savedDocs;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('skill bank initialization logs an unreadable seed document without aborting startup', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-seed-'));
  const savedBank = process.env.GAH_SKILL_BANK_PATH;
  const savedDocs = process.env.GAH_SKILL_DOCS_PATH;
  const savedConsoleError = console.error;
  const errors: unknown[][] = [];
  process.env.GAH_SKILL_BANK_PATH = join(dir, 'missing-skills.json');
  process.env.GAH_SKILL_DOCS_PATH = dir;
  console.error = (...args: unknown[]) => errors.push(args);

  try {
    assert.doesNotThrow(() => initializeSkillBank());
    assert.equal(errors[0]?.[0], 'Failed to seed skill bank from docs:');
    assert.match(String(errors[0]?.[1]), /EISDIR|directory/i);
  } finally {
    console.error = savedConsoleError;
    if (savedBank === undefined) delete process.env.GAH_SKILL_BANK_PATH;
    else process.env.GAH_SKILL_BANK_PATH = savedBank;
    if (savedDocs === undefined) delete process.env.GAH_SKILL_DOCS_PATH;
    else process.env.GAH_SKILL_DOCS_PATH = savedDocs;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('server startup from apps/server seeds the repo manager skill content intact', () => {
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-default-seed-'));
  const bankPath = join(dir, 'skills.json');
  const savedBank = process.env.GAH_SKILL_BANK_PATH;
  const savedDocs = process.env.GAH_SKILL_DOCS_PATH;
  const savedCwd = process.cwd();
  process.env.GAH_SKILL_BANK_PATH = bankPath;
  delete process.env.GAH_SKILL_DOCS_PATH;

  try {
    process.chdir(fileURLToPath(new URL('..', import.meta.url)));
    initializeSkillBank();
    const bank = JSON.parse(readFileSync(bankPath, 'utf8')) as { skills: Array<{ id: string; content: string }> };
    const expected = readFileSync(
      fileURLToPath(new URL('../../../docs/gah-manager-skill.md', import.meta.url)),
      'utf8'
    );
    assert.equal(bank.skills.find((skill) => skill.id === 'gah-manager')?.content, expected);
  } finally {
    process.chdir(savedCwd);
    if (savedBank === undefined) delete process.env.GAH_SKILL_BANK_PATH;
    else process.env.GAH_SKILL_BANK_PATH = savedBank;
    if (savedDocs === undefined) delete process.env.GAH_SKILL_DOCS_PATH;
    else process.env.GAH_SKILL_DOCS_PATH = savedDocs;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('skill bank and git-commit mutation routes reject unauthenticated non-loopback requests like /api/projects', async () => {
  const { isLocalAddress } = await import('./authMiddleware.js');
  const dir = mkdtempSync(join(tmpdir(), 'gah-skills-api-'));
  const savedBank = process.env.GAH_SKILL_BANK_PATH;
  process.env.GAH_SKILL_BANK_PATH = join(dir, 'skills.json');
  const os = await import('node:os');
  const app = createServer({});
  const server = http.createServer(app);
  await new Promise<void>((resolve) => server.listen(0, '0.0.0.0', () => resolve()));
  const { port } = server.address() as AddressInfo;
  const nonLoopbackIp = Object.values(os.networkInterfaces())
    .flatMap((list) => list ?? [])
    .find((addr) => addr?.family === 'IPv4' && !isLocalAddress(addr.address))
    ?.address;
  try {
    if (!nonLoopbackIp) return;
    const baseUrl = `http://${nonLoopbackIp}:${port}`;
    // No TLS, no token -> 403 Forbidden, exactly like /api/projects.
    for (const path of ['/api/skills', '/api/projects', '/api/admin/update', '/api/git/commit']) {
      const res = await fetch(`${baseUrl}${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: 'x', version: '1.0.0', content: 'c' })
      });
      assert.equal(res.status, 403, `${path} must be gated by authMiddleware`);
    }
  } finally {
    if (savedBank === undefined) delete process.env.GAH_SKILL_BANK_PATH;
    else process.env.GAH_SKILL_BANK_PATH = savedBank;
    rmSync(dir, { recursive: true, force: true });
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

async function withAdminUpdateEnv(value: string | undefined, fn: () => Promise<void>): Promise<void> {
  const saved = process.env.GAH_ENABLE_ADMIN_UPDATE;
  if (value === undefined) delete process.env.GAH_ENABLE_ADMIN_UPDATE;
  else process.env.GAH_ENABLE_ADMIN_UPDATE = value;
  try {
    await fn();
  } finally {
    if (saved === undefined) delete process.env.GAH_ENABLE_ADMIN_UPDATE;
    else process.env.GAH_ENABLE_ADMIN_UPDATE = saved;
  }
}

test('/api/admin/update is 404 (not just unimplemented) without GAH_ENABLE_ADMIN_UPDATE=1', async () => {
  await withAdminUpdateEnv(undefined, async () => {
    await withTestServer(
      async (profile) => profilePayload(profile),
      async (baseUrl) => {
        const getRes = await fetch(`${baseUrl}/api/admin/update`);
        assert.equal(getRes.status, 404);
        const postRes = await fetch(`${baseUrl}/api/admin/update`, { method: 'POST' });
        assert.equal(postRes.status, 404);
        const statusRes = await fetch(`${baseUrl}/api/admin/update/status`);
        assert.equal(statusRes.status, 404);
      }
    );
  });
});

test('GET /api/admin/update returns the injected pending-commits check when enabled', async () => {
  await withAdminUpdateEnv('1', async () => {
    const pending = {
      current: { hash: 'aaa', short: 'aaa', subject: 'current' },
      latest: { hash: 'bbb', short: 'bbb', subject: 'latest' },
      commitsBehind: 2,
      upToDate: false
    };
    const app = createServer({ getPendingCommits: () => pending });
    const server = http.createServer(app);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const { port } = server.address() as AddressInfo;
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/admin/update`);
      assert.equal(res.status, 200);
      assert.deepEqual(await res.json(), pending);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

test('POST /api/admin/update launches the update via the injected helper and returns 202', async () => {
  await withAdminUpdateEnv('1', async () => {
    const runningState = {
      status: 'running' as const,
      startedAt: '2026-01-01T00:00:00.000Z',
      finishedAt: null,
      exitCode: null,
      pid: 1234,
      output: ''
    };
    let startCalled = false;
    const app = createServer({
      startAdminUpdate: () => {
        startCalled = true;
        return { started: true, state: runningState };
      }
    });
    const server = http.createServer(app);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const { port } = server.address() as AddressInfo;
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/admin/update`, { method: 'POST' });
      assert.equal(res.status, 202);
      assert.equal(startCalled, true);
      assert.deepEqual(await res.json(), runningState);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

test('POST /api/admin/update returns 409 when an update is already running', async () => {
  await withAdminUpdateEnv('1', async () => {
    const runningState = {
      status: 'running' as const,
      startedAt: '2026-01-01T00:00:00.000Z',
      finishedAt: null,
      exitCode: null,
      pid: 1234,
      output: ''
    };
    const app = createServer({ startAdminUpdate: () => ({ started: false, state: runningState }) });
    const server = http.createServer(app);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const { port } = server.address() as AddressInfo;
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/admin/update`, { method: 'POST' });
      assert.equal(res.status, 409);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

test('GET /api/admin/update/status returns reconnect-safe state via the injected reader', async () => {
  await withAdminUpdateEnv('1', async () => {
    const inferredState = {
      status: 'inferred_restart' as const,
      startedAt: '2026-01-01T00:00:00.000Z',
      finishedAt: '2026-01-01T00:05:00.000Z',
      exitCode: null,
      pid: 1234,
      output: 'build output...'
    };
    const app = createServer({ readAdminUpdateState: () => inferredState });
    const server = http.createServer(app);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const { port } = server.address() as AddressInfo;
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/admin/update/status`);
      assert.equal(res.status, 200);
      assert.deepEqual(await res.json(), inferredState);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

/** POST /api/git/commit must never fall back to the shared profile checkout
 * for a sessionId that doesn't resolve to a live, writable session -- an
 * unknown/archived session or a worktree-less read-only PR chat has to be
 * rejected outright instead of silently committing into local_path. */
test('POST /api/git/commit rejects unknown, archived, and read-only PR sessions instead of falling back to the profile checkout', { timeout: 30_000 }, async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-git-commit-route-'));
  const checkout = join(root, 'checkout');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  execFileSync('git', ['config', 'user.email', 't@t'], { cwd: checkout });
  execFileSync('git', ['config', 'user.name', 't'], { cwd: checkout });
  writeFileSync(join(checkout, 'README.md'), '# repo\n');
  execFileSync('git', ['add', '.'], { cwd: checkout });
  execFileSync('git', ['commit', '--quiet', '-m', 'init'], { cwd: checkout });

  const profile = 'git-commit-route';
  const profileListPath = join(root, 'profile-list.json');
  const worktreeBase = join(root, 'worktrees');
  writeFileSync(profileListPath, JSON.stringify([{
    name: profile,
    display_name: 'Git Commit Route',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase,
    web_url: 'https://github.com/owner/repo',
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }]));

  const savedEnv = { ...process.env };
  process.env.GAH_BINARY = join(gitFixtures, 'gah', 'gah');
  process.env.GAH_FIXTURE_PROFILE_LIST = profileListPath;
  process.env.GAH_CHAT_STATE_DIR = join(root, 'chat');
  process.env.GAH_GATEWAY_SETTINGS_PATH = join(root, 'gateway.json');
  process.env.GAH_MANAGER_CHAT_SETTINGS_PATH = join(root, 'manager-chat.json');
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'coordinator-identity.json');
  resetCachedCoordinatorIdentity();

  const profileInfo = {
    repo_id: 'repo',
    local_path: checkout,
    worktree_base: worktreeBase
  };

  const app = createServer({});
  const server = http.createServer(app);
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  try {
    const commit = (sessionId: string) => fetch(`${baseUrl}/api/git/commit?profile=${profile}&sessionId=${sessionId}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: 'should not land' })
    });

    const unknownResponse = await commit('does-not-exist');
    assert.equal(unknownResponse.status, 404, 'unknown session id must be rejected, not routed to local_path');

    const archived = await createSession(
      { profile, profileInfo, backend: 'codex', worktree: false },
      { stateDir: join(root, 'chat') }
    );
    await archiveSession(profile, archived.id, profileInfo, { stateDir: join(root, 'chat') });
    const archivedResponse = await commit(archived.id);
    assert.equal(archivedResponse.status, 404, 'archived session must be rejected, not routed to local_path');

    const prSession = await createSession(
      { profile, profileInfo, backend: 'codex', prNumber: 7, worktree: false },
      { stateDir: join(root, 'chat') }
    );
    assert.equal(prSession.worktreePath, null, 'PR chat is worktree-less by design');
    const prResponse = await commit(prSession.id);
    assert.equal(prResponse.status, 403, 'worktree-less read-only PR session must be rejected, not routed to local_path');

    // Confirm none of the rejected attempts ever touched local_path.
    const log = execFileSync('git', ['log', '--oneline'], { cwd: checkout }).toString();
    assert.equal(log.trim().split('\n').length, 1, 'only the initial commit exists in the shared checkout');
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    process.env = savedEnv;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  }
});
