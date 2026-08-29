// Issue #635: HTTP endpoint contract tests against the real Express app and
// the real gahCli.ts spawn/parse/error logic -- only the underlying `gah`
// binary is replaced, via GAH_BINARY pointed at the fake fixture under
// tests/fixtures/gah. This is the part of server.ts's behavior that can't be
// exercised through createServer()'s DI overrides (runStatus/runQuota/
// runReport/runConfigShow/runProfileList/runProfileAdd are direct top-level
// imports, not injectable).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { StatusSnapshot, QuotaSnapshot, ReportData, ProfileSummary } from '@git-agent-harness/contracts';

import { withFixtureServer, uniqueFixtureProfile } from './fixtureGahHarness.js';

test('GET /api/status returns 200 with a parseable typed payload from the real CLI shape', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/status?profile=${uniqueFixtureProfile()}`);
    const body = (await response.json()) as StatusSnapshot;

    assert.equal(response.status, 200);
    assert.equal(body.schema_version, 1);
    assert.equal(body.profile.repo_id, 'fixture-repo');
    assert.deepEqual(body.errors, []);
  });
});

test('GET /api/quota returns 200 with a parseable typed payload', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/quota?profile=${uniqueFixtureProfile()}`);
    const body = (await response.json()) as QuotaSnapshot;

    assert.equal(response.status, 200);
    assert.equal(body.schema_version, 2);
    assert.equal(body.profile.repo_id, 'fixture-repo');
    assert.equal(body.freshness.quota_checked_at, '2099-08-07T15:20:00Z');
    assert.equal(body.freshness.quota_observed_at, '2026-08-07T15:18:10.130973215Z');
    assert.deepEqual(body.quota_checks, [
      {
        backend: 'codex',
        checked_at: '2099-08-07T15:20:00Z',
        status: 'no_data'
      },
      {
        backend: 'vibe',
        checked_at: '2026-08-07T15:19:00Z',
        status: 'failed',
        error: 'Mistral Admin API unavailable'
      }
    ]);
  });
});

test('GET /api/report returns 200 with a parseable typed payload', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/report`);
    const body = (await response.json()) as ReportData;

    assert.equal(response.status, 200);
    assert.equal(body.total_entries, 42);
    assert.equal(body.group_by, 'Backend');
    assert.ok(body.comparisons.length >= 2, 'fixture report should carry backend comparison rows');
  });
});

test('GET /api/config returns 200 with a parseable typed payload', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/config`);
    const body = (await response.json()) as { current_manager: string | null };

    assert.equal(response.status, 200);
    assert.equal(body.current_manager, null);
  });
});

test('GET /api/profiles returns 200 with a parseable typed payload', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/profiles`);
    const body = (await response.json()) as ProfileSummary[];

    assert.equal(response.status, 200);
    assert.equal(body.length, 1);
    assert.equal(body[0]?.name, 'fixture');
  });
});

// AC2: a non-zero CLI exit must surface as an HTTP error carrying the
// stderr message, never a healthy-looking 200 with empty/default data.
test('a non-zero CLI exit on /api/status surfaces as 502 with the stderr message, not empty data', async () => {
  await withFixtureServer(
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/status?profile=${uniqueFixtureProfile()}`);
      const body = (await response.json()) as { error?: string; message?: string };

      assert.equal(response.status, 502);
      assert.equal(body.error, 'Failed to load gah status');
      assert.match(body.message ?? '', /profile fixture is on fire/);
    },
    { command: 'status', message: 'profile fixture is on fire' }
  );
});

test('a non-zero CLI exit on /api/quota surfaces as 502 with the stderr message', async () => {
  await withFixtureServer(
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/quota?profile=${uniqueFixtureProfile()}`);
      const body = (await response.json()) as { error?: string; message?: string };

      assert.equal(response.status, 502);
      assert.match(body.message ?? '', /quota backend unreachable/);
    },
    { command: 'quota', message: 'quota backend unreachable' }
  );
});

test('a non-zero CLI exit on /api/profiles (list) surfaces as 502 with the stderr message', async () => {
  await withFixtureServer(
    async (baseUrl) => {
      const response = await fetch(`${baseUrl}/api/profiles`);
      const body = (await response.json()) as { error?: string; message?: string };

      assert.equal(response.status, 502);
      assert.match(body.message ?? '', /profile store corrupted/);
    },
    { command: 'profile-list', message: 'profile store corrupted' }
  );
});

// AC3: a POST /api/profiles missing required fields must fail closed with
// 4xx, before ever reaching the CLI -- covered without needing the fixture
// to understand `profile add` at all, since the rejection must happen
// first.
test('POST /api/profiles with a missing required field is rejected with 400, never reaching the CLI', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/profiles`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: 'incomplete',
        display_name: 'Incomplete'
        // repo_id, provider, repo, local_path, artifact_root all missing
      })
    });
    const body = (await response.json()) as { error?: string; message?: string };

    assert.equal(response.status, 400);
    assert.match(body.message ?? '', /repo_id/);
    assert.match(body.message ?? '', /artifact_root/);
  });
});

test('POST /api/profiles with an empty-string required field is also rejected with 400', async () => {
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/profiles`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: 'blank',
        display_name: 'Blank',
        repo_id: '',
        provider: 'github',
        repo: 'owner/repo',
        local_path: '/tmp/repo',
        artifact_root: '/tmp/artifacts'
      })
    });
    const body = (await response.json()) as { error?: string; message?: string };

    assert.equal(response.status, 400);
    assert.match(body.message ?? '', /repo_id/);
  });
});

test('POST /api/profiles with all required fields present reaches the CLI (past validation)', async () => {
  // The fixture doesn't implement `profile add`, so this exercises "past
  // validation, into the CLI" via the fixture's generic unrecognized-command
  // failure (exit 127) rather than a real add -- proving the validation
  // gate above does NOT reject a fully-populated request.
  await withFixtureServer(async (baseUrl) => {
    const response = await fetch(`${baseUrl}/api/profiles`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: 'complete',
        display_name: 'Complete',
        repo_id: 'complete-repo',
        provider: 'github',
        repo: 'owner/complete',
        local_path: '/tmp/complete',
        artifact_root: '/tmp/complete-artifacts'
      })
    });

    assert.equal(response.status, 502, 'validation must not reject a fully-populated request');
  });
});
