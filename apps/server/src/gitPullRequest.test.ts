import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { withFixtureServer } from './fixtureGahHarness.js';

test('Git page creates provider-scoped PRs and MRs through isolated CLI fixtures', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'gah-gitlab-create-'));
  execFileSync('git', ['init', '--quiet', '--initial-branch=feature/dashboard', root]);
  const log = join(root, 'calls.jsonl');
  const mode = join(root, 'mode');
  const profiles = join(root, 'profiles.json');
  const baseline = JSON.parse(readFileSync(new URL('../tests/fixtures/gah/responses/profile-list.json', import.meta.url), 'utf8'))[0];
  const repo = 'group/subgroup/project';
  const webUrl = `https://gitlab.example.test:8443/${repo}`;
  writeFileSync(profiles, JSON.stringify([
    { ...baseline, name: 'gitlab', provider: 'gitlab', repo, local_path: root, web_url: webUrl },
    { ...baseline, name: 'github', provider: 'github', repo: 'owner/repo', local_path: root, web_url: 'https://github.com/owner/repo' },
    { ...baseline, name: 'invalid-host', provider: 'gitlab', repo, local_path: root, web_url: `http://gitlab.example.test/${repo}` }
  ]));
  const fake = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({command, args}) + '\\n');
if (command === 'gh') { process.stdout.write('https://github.com/owner/repo/pull/7\\n'); process.exit(0); }
const mode = fs.existsSync(${JSON.stringify(mode)}) ? fs.readFileSync(${JSON.stringify(mode)}, 'utf8') : '';
if (mode === 'failure') { process.stderr.write('test-provider-secret'); process.exit(1); }
if (mode === 'invalid-json') { process.stdout.write('test-provider-secret'); process.exit(0); }
if (mode === 'api-error') { process.stdout.write(JSON.stringify({message:'test-provider-secret'})); process.exit(0); }
if (mode === 'wrong-host') { process.stdout.write(JSON.stringify({iid:7,web_url:'https://other.example.test/group/subgroup/project/-/merge_requests/7'})); process.exit(0); }
if (mode === 'wrong-project') { process.stdout.write(JSON.stringify({iid:7,web_url:'https://gitlab.example.test:8443/other/project/-/merge_requests/7'})); process.exit(0); }
if (mode === 'no-default') { process.stdout.write(JSON.stringify({default_branch:null})); process.exit(0); }
const method = args[args.indexOf('--method') + 1];
process.stdout.write(JSON.stringify(method === 'GET' ? {default_branch:'trunk'} : {iid:7, web_url:${JSON.stringify(webUrl + '/-/merge_requests/7')}}));
`;
  writeFileSync(join(root, 'gh'), fake, { mode: 0o755 });
  writeFileSync(join(root, 'glab'), fake, { mode: 0o755 });
  const savedPath = process.env.PATH;
  const savedProfiles = process.env.GAH_FIXTURE_PROFILE_LIST;
  process.env.PATH = `${root}:${savedPath}`;
  process.env.GAH_FIXTURE_PROFILE_LIST = profiles;
  t.after(() => {
    process.env.PATH = savedPath;
    if (savedProfiles === undefined) delete process.env.GAH_FIXTURE_PROFILE_LIST;
    else process.env.GAH_FIXTURE_PROFILE_LIST = savedProfiles;
    rmSync(root, { recursive: true, force: true });
  });
  const calls = () => readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line));
  await withFixtureServer(async (baseUrl) => {
    const create = (profile: string, body: unknown) => fetch(`${baseUrl}/api/git/pr?profile=${profile}`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body)
    });
    await t.test('custom-host nested namespace uses glab with explicit host, source, base, body and draft title', async () => {
      writeFileSync(log, '');
      const response = await create('gitlab', { title: 'Dashboard MR', body: '--literal body', base: 'release', draft: true });
      assert.equal(response.status, 200);
      assert.deepEqual(await response.json(), { url: `${webUrl}/-/merge_requests/7` });
      assert.deepEqual(calls(), [{ command: 'glab', args: [
        'api', 'projects/group%2Fsubgroup%2Fproject/merge_requests', '--hostname', 'gitlab.example.test:8443', '--method', 'POST',
        '--raw-field', 'source_branch=feature/dashboard', '--raw-field', 'target_branch=release',
        '--raw-field', 'title=Draft: Dashboard MR', '--raw-field', 'description=--literal body'
      ] }]);
    });
    await t.test('omitted base uses the configured project default on the same host', async () => {
      writeFileSync(log, '');
      const response = await create('gitlab', { title: 'Dashboard MR' });
      assert.equal(response.status, 200);
      const recorded = calls();
      assert.deepEqual(recorded[0], { command: 'glab', args: ['api', 'projects/group%2Fsubgroup%2Fproject', '--hostname', 'gitlab.example.test:8443', '--method', 'GET'] });
      assert.ok(recorded[1].args.includes('target_branch=trunk'));
      assert.ok(recorded[1].args.includes('title=Dashboard MR'));
      assert.ok(recorded[1].args.includes('description='));
    });
    await t.test('GitHub keeps the existing gh create flags', async () => {
      writeFileSync(log, '');
      const response = await create('github', { title: 'Dashboard PR', body: 'Body', base: 'release', draft: true });
      assert.equal(response.status, 200);
      assert.deepEqual(await response.json(), { url: 'https://github.com/owner/repo/pull/7' });
      assert.deepEqual(calls(), [{ command: 'gh', args: ['pr', 'create', '--title', 'Dashboard PR', '--body', 'Body', '--base', 'release', '--draft'] }]);
    });
    await t.test('an existing draft prefix is preserved and invalid fields never invoke a provider', async () => {
      writeFileSync(log, '');
      const response = await create('gitlab', { title: 'Draft: Dashboard MR', base: 'main', draft: true });
      assert.equal(response.status, 200);
      assert.ok(calls()[0].args.includes('title=Draft: Dashboard MR'));
      writeFileSync(log, '');
      for (const body of [{}, { title: 7 }, { title: ' ' }, { title: 'MR', body: {} }, { title: 'MR', base: [] }, { title: 'MR', draft: 'true' }]) {
        const invalid = await create('gitlab', body);
        assert.equal(invalid.status, 400);
      }
      assert.deepEqual(calls(), []);
    });
    await t.test('CLI/API failures and invalid host configuration fail without exposing provider output', async () => {
      for (const failure of ['failure', 'invalid-json', 'api-error', 'wrong-host', 'wrong-project']) {
        writeFileSync(mode, failure);
        const response = await create('gitlab', { title: 'Dashboard MR', base: 'main' });
        assert.equal(response.status, 502);
        assert.ok(!(await response.text()).includes('test-provider-secret'));
      }
      writeFileSync(mode, 'no-default');
      writeFileSync(log, '');
      const noDefault = await create('gitlab', { title: 'Dashboard MR' });
      assert.equal(noDefault.status, 502);
      assert.equal(calls().length, 1, 'missing default branch must not create an MR');
      assert.ok(calls()[0].args.includes('GET'));
      writeFileSync(mode, '');
      writeFileSync(log, '');
      const response = await create('invalid-host', { title: 'Dashboard MR', base: 'main' });
      assert.equal(response.status, 502);
      assert.deepEqual(calls(), []);
    });
  });
});
