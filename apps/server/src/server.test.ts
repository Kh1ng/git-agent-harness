import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';
import type { ConfigProfileSummary, DoctorSnapshot, ProfileSummary } from '@git-agent-harness/contracts';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

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
    const body = (await response.json()) as ConfigProfileSummary;

    assert.equal(response.status, 200);
    assert.equal(body.profile, 'repo');
    assert.equal(calledProfile, 'repo');
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
      const body = (await response.json()) as ConfigProfileSummary;

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
    local_path: '/repos/repo',
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
    await withTestServer(async () => profilePayload('alpha'), async (baseUrl) => {
      const empty = (await (await fetch(`${baseUrl}/api/skills`)).json()) as { skills: unknown[] };
      assert.deepEqual(empty.skills, []);

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

test('skill bank mutation routes reject unauthenticated non-loopback requests like /api/projects', async () => {
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
    for (const path of ['/api/skills', '/api/projects']) {
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
