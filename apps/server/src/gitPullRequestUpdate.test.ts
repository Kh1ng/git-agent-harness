import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { createServer } from './server.js';
import { resetCachedCoordinatorIdentity } from './coordinatorIdentity.js';

test('published review edits are authenticated, validated, idempotent, audited, and surface provider stderr', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'gah-pr-update-'));
  const checkout = join(root, 'checkout');
  const log = join(root, 'calls.jsonl');
  const mode = join(root, 'mode');
  execFileSync('mkdir', ['-p', checkout]);
  execFileSync('git', ['init', '--quiet', '--initial-branch=main'], { cwd: checkout });
  const profiles = join(root, 'profiles.json');
  writeFileSync(profiles, JSON.stringify([{
    name: 'review-edit', display_name: 'Review edit', provider: 'github', repo: 'owner/repo', repo_id: 'repo', local_path: checkout,
    worktree_base: join(root, 'worktrees'), web_url: 'https://github.com/owner/repo', max_parallel_workers: null, max_open_managed_mrs: 1,
    manager_wake_autonomy: null, validation_timeout_seconds: 300,
  }]));
  writeFileSync(join(root, 'gh'), `#!/usr/bin/env node
const fs = require('node:fs');
fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify(process.argv.slice(2)) + '\\n');
if (fs.existsSync(${JSON.stringify(mode)})) { process.stderr.write('provider exploded'); process.exit(1); }
`, { mode: 0o755 });

  const saved = { ...process.env };
  process.env.GAH_BINARY = resolve(dirname(fileURLToPath(import.meta.url)), '../tests/fixtures/gah/gah');
  process.env.PATH = `${root}:${process.env.PATH}`;
  process.env.GAH_FIXTURE_PROFILE_LIST = profiles;
  process.env.GAH_MUTATION_STORE_PATH = join(root, 'mutations');
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(root, 'identity.json');
  process.env.COORDINATOR_TOKEN = 'review-edit-token';
  process.env.GAH_ALLOW_INSECURE_HTTP = '1';
  resetCachedCoordinatorIdentity();
  t.after(() => {
    process.env = saved;
    resetCachedCoordinatorIdentity();
    rmSync(root, { recursive: true, force: true });
  });

  const server = http.createServer(createServer());
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise<void>(resolve => server.close(() => resolve())));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const remote = { 'X-Forwarded-For': '198.51.100.10', 'Content-Type': 'application/json' };
  const post = (key: string, body: unknown, authenticate = true) => fetch(`${base}/api/git/pull-request/update?profile=review-edit`, {
    method: 'POST',
    headers: { ...remote, ...(authenticate ? { Authorization: 'Bearer review-edit-token' } : {}), 'Idempotency-Key': key },
    body: JSON.stringify(body),
  });

  assert.equal((await post('unauthorized-edit-0001', { number: 12, title: 'Title', body: '' }, false)).status, 401);
  assert.equal((await post('invalid-review-edit-01', { number: 0, title: '', body: 4 })).status, 400);

  const accepted = await post('review-edit-success-01', { number: 12, title: 'Better title', body: 'Better body' });
  assert.equal(accepted.status, 200);
  assert.deepEqual(await accepted.json(), { url: 'https://github.com/owner/repo/pull/12' });
  assert.equal((await post('review-edit-success-01', { number: 12, title: 'Better title', body: 'Better body' })).status, 409);
  assert.deepEqual(readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line)), [
    ['pr', 'edit', '12', '--title', 'Better title', '--body', 'Better body'],
  ]);
  const audit = readFileSync(join(root, 'mutations/audit.jsonl'), 'utf8');
  assert.match(audit, /"operation":"git.update"/);
  assert.match(audit, /"result":"completed"/);

  writeFileSync(mode, 'failure');
  const failed = await post('review-edit-failure-01', { number: 12, title: 'Still better', body: 'Body' });
  assert.equal(failed.status, 502);
  assert.match((await failed.json() as { message: string }).message, /provider exploded/);
});
