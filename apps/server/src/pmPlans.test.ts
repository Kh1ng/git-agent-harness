import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import type { PmPlanDetail, PmPlanList, PmPlanOperation } from '@git-agent-harness/contracts';
import { withFixtureServer } from './fixtureGahHarness.js';

function fixture(provider: string): PmPlanDetail {
  return JSON.parse(readFileSync(new URL(`../../../packages/contracts/src/fixtures/pm-plan-${provider}.json`, import.meta.url), 'utf8'));
}

test('PM endpoints preserve shared GitHub/GitLab artifacts and partial state through the CLI adapter', async () => {
  await withFixtureServer(async base => {
    for (const provider of ['github', 'gitlab']) {
      const expected = fixture(provider);
      const query = `?profile=${expected.profile}`;
      const listed = await fetch(`${base}/api/pm/plans${query}&limit=1`);
      assert.equal(listed.status, 200);
      assert.equal(listed.headers.get('cache-control'), 'no-store');
      const list = await listed.json() as PmPlanList;
      assert.equal(list.plans[0].publication_status, expected.publication.status);
      assert.equal(list.plans[0].plan_fingerprint, expected.publication.plan_fingerprint);
      const detail = await fetch(`${base}/api/pm/plans/plan-1${query}`);
      assert.equal(detail.status, 200);
      assert.deepEqual(await detail.json(), expected);
      const action = async (name: string, body: unknown) => fetch(`${base}/api/pm/plans/plan-1/${name}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
      });
      const dryRun = await action('dry-run', { profile: expected.profile });
      assert.equal(dryRun.status, 200);
      assert.equal((await dryRun.json() as PmPlanOperation).dry_run, true);
      const failed = await action('publish', { profile: expected.profile, approve: true, plan_fingerprint: expected.publication.plan_fingerprint });
      assert.equal(failed.status, 409);
      const failure = await failed.json() as PmPlanOperation;
      assert.equal(failure.success, false);
      assert.deepEqual(failure.plan, expected);
      assert.match(failure.error ?? '', /stopped after the first child/);
    }
  });
});

test('PM API rejects unapproved publication, paths, extra arguments, and unauthenticated proxy requests before CLI work', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'gah-pm-api-'));
  const saved = { log: process.env.GAH_FIXTURE_PM_LOG, token: process.env.COORDINATOR_TOKEN, insecure: process.env.GAH_ALLOW_INSECURE_HTTP };
  const log = join(directory, 'argv.jsonl');
  process.env.GAH_FIXTURE_PM_LOG = log;
  process.env.COORDINATOR_TOKEN = 'fake-pm-token';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  try {
    await withFixtureServer(async base => {
      const expected = fixture('github');
      const body = { profile: expected.profile, approve: true, plan_fingerprint: expected.publication.plan_fingerprint };
      const request = (path: string, input: unknown, headers: Record<string, string> = {}) => fetch(`${base}/api/pm/${path}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify(input),
      });
      for (const input of [{ profile: body.profile }, { ...body, approve: false }, { ...body, plan_fingerprint: 'stale' }, { ...body, plan: '/tmp/foreign.json' }, { ...body, config: '/tmp/foreign.toml' }]) {
        assert.equal((await request('plans/plan-1/publish', input)).status, 400);
      }
      for (const id of ['a%2Fb', '%2e%2e%2fforeign', 'a%5Cb']) {
        assert.equal((await request(`plans/${id}/publish`, body)).status, 400);
      }
      const rejected = await request('plans/plan-1/publish', body, { 'X-Forwarded-For': '198.51.100.1' });
      assert.equal(rejected.status, 401);
      assert.throws(() => readFileSync(log), /ENOENT/);
      assert.equal((await fetch(`${base}/api/pm/plans?profile=unknown`)).status, 422);
      const approved = await request('plans/plan-1/publish', body, { 'X-Forwarded-For': '198.51.100.1', Authorization: 'Bearer fake-pm-token' });
      assert.equal(approved.status, 409, 'Authorized request reaches the fixture publisher');
      const calls = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line) as string[]);
      assert.equal(calls.length, 2);
      assert.deepEqual(calls[1].slice(0, 6), ['pm', 'publish', '--profile=github-project', '--json', '--plan-id=plan-1', `--expected-fingerprint=${body.plan_fingerprint}`]);
      for (let i = 0; i < 35; i++) await fetch(`${base}/api/pm/plans?profile=github-project`);
      assert.equal((await fetch(`${base}/api/pm/plans?profile=github-project`)).status, 429);
    });
  } finally {
    for (const [key, value] of Object.entries({ GAH_FIXTURE_PM_LOG: saved.log, COORDINATOR_TOKEN: saved.token, GAH_ALLOW_INSECURE_HTTP: saved.insecure })) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(directory, { recursive: true });
  }
});
