import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from './server.js';
import type { ConfigProfileSummary, DoctorSnapshot } from '@git-agent-harness/contracts';
import http from 'node:http';
import type { AddressInfo } from 'node:net';

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
  runDoctor?: (profile: string) => Promise<DoctorSnapshot>
) {
  const app = createServer({
    runConfigShowProfile: runProfile,
    ...(runDoctor ? { runDoctor } : {})
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
