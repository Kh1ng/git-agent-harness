import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, test } from 'node:test';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import type { ProfileAddOptions } from './gahCli.js';
import { addProject, addRemoteProject, importGitProject, listProjects, parseGitUrl, removeProject, resolveChatProject } from './projectCatalog.js';

const savedCatalogPath = process.env.GAH_PROJECT_CATALOG_PATH;
const savedEnvironment = new Map(
  ['GAH_PROJECTS_ROOT', 'XDG_DATA_HOME', 'GAH_COORDINATOR_IDENTITY_PATH']
    .map((name) => [name, process.env[name]])
);

beforeEach(() => {
  process.env.GAH_COORDINATOR_IDENTITY_PATH = join(mkdtempSync(join(tmpdir(), 'gah-project-identity-')), 'identity.json');
  writeFileSync(process.env.GAH_COORDINATOR_IDENTITY_PATH, JSON.stringify({ node_id: 'local' }));
});

afterEach(() => {
  if (savedCatalogPath === undefined) delete process.env.GAH_PROJECT_CATALOG_PATH;
  else process.env.GAH_PROJECT_CATALOG_PATH = savedCatalogPath;
  for (const [name, value] of savedEnvironment) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
});

function profile(name: string): ProfileSummary {
  return {
    name,
    display_name: name,
    provider: 'github',
    repo: `owner/${name}`,
    repo_id: name,
    local_path: `/repos/${name}`,
    worktree_base: `/worktrees`,
    web_url: `https://github.com/owner/${name}`,
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  };
}

test('catalog lists only explicitly added profiles', () => {
  process.env.GAH_PROJECT_CATALOG_PATH = join(mkdtempSync(join(tmpdir(), 'gah-projects-')), 'projects.json');
  const profiles = [profile('alpha'), profile('beta')];

  assert.deepEqual(listProjects(profiles), []);
  assert.equal(addProject('beta', profiles).name, 'beta');
  assert.deepEqual(listProjects(profiles).map((item) => item.name), ['beta']);
  assert.equal(removeProject('beta'), true);
  assert.deepEqual(listProjects(profiles), []);
});

test('catalog rejects a profile that is not configured', () => {
  process.env.GAH_PROJECT_CATALOG_PATH = join(mkdtempSync(join(tmpdir(), 'gah-projects-')), 'projects.json');
  assert.throws(() => addProject('missing', [profile('alpha')]), /not configured/);
});

test('git import clones, derives a profile, re-clones clean checkouts, and guards dirty work', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-project-import-'));
  const source = join(root, 'source');
  const projectsRoot = join(root, 'projects');
  execFileSync('git', ['init', '--initial-branch=main', source]);
  writeFileSync(join(source, 'package.json'), JSON.stringify({ scripts: { test: 'node --test', typecheck: 'tsc --noEmit' } }));
  execFileSync('git', ['-C', source, 'add', 'package.json']);
  execFileSync('git', ['-C', source, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-m', 'initial']);

  process.env.GAH_PROJECTS_ROOT = projectsRoot;
  process.env.XDG_DATA_HOME = join(root, 'data');
  const remoteUrl = 'https://github.com/owner/repo.git';
  const git = async (args: string[]): Promise<string> => {
    const cloneUrlIndex = args.indexOf(remoteUrl);
    const effectiveArgs = [...args];
    if (cloneUrlIndex >= 0) effectiveArgs[cloneUrlIndex] = source;
    const output = execFileSync('git', effectiveArgs, { encoding: 'utf8' }).trim();
    if (cloneUrlIndex >= 0) {
      execFileSync('git', ['-C', args.at(-1)!, 'remote', 'set-url', 'origin', remoteUrl]);
    }
    return output;
  };

  let added: ProfileAddOptions | undefined;
  let addCalls = 0;
  let configured: ProfileSummary[] = [];
  const dependencies = {
    listProfiles: async () => configured,
    addProfile: async (options: ProfileAddOptions) => {
      added = options;
      addCalls += 1;
      configured = [{
        name: options.name,
        display_name: options.display_name,
        provider: options.provider,
        repo: options.repo,
        repo_id: options.name,
        local_path: options.local_path,
        worktree_base: '/worktrees',
        web_url: remoteUrl.replace(/\.git$/, ''),
        max_parallel_workers: null,
        max_open_managed_mrs: 1,
        manager_wake_autonomy: null,
        validation_timeout_seconds: 300
      }];
    },
    git
  };

  const imported = await importGitProject({ gitUrl: remoteUrl }, dependencies);
  assert.equal(imported.checkoutStatus, 'cloned');
  assert.equal(added?.repo, 'owner/repo');
  assert.equal(added?.default_target_branch, 'main');
  assert.deepEqual(added?.validation_commands, ['npm test', 'npm run typecheck']);
  assert.equal(existsSync(join(projectsRoot, 'owner-repo', '.git')), true);

  const recloned = await importGitProject({ gitUrl: remoteUrl, reclone: true }, dependencies);
  assert.equal(recloned.checkoutStatus, 'recloned');
  assert.equal(addCalls, 1);

  writeFileSync(join(projectsRoot, 'owner-repo', 'dirty.txt'), 'do not delete');
  await assert.rejects(
    importGitProject({ gitUrl: remoteUrl, reclone: true }, dependencies),
    /uncommitted changes/
  );
});

test('git import accepts hosted Git URLs only', () => {
  assert.equal(parseGitUrl('git@github.com:owner/repo.git').repo, 'owner/repo');
  assert.throws(() => parseGitUrl('file:///tmp/repo'), /GitHub or GitLab/);
  assert.throws(() => parseGitUrl('https://user:secret@github.com/owner/repo.git'), /credentials/);
  assert.throws(() => parseGitUrl('https://github.com/owner/repo.git?token=secret'), /query parameters/);
});

test('git import of an existing profile with a missing checkout clones into the configured path', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-project-import-'));
  const source = join(root, 'source');
  const projectsRoot = join(root, 'projects');
  const missingCheckout = join(root, 'unmanaged', 'repo'); // outside the managed root, does not exist
  execFileSync('git', ['init', '--initial-branch=main', source]);
  writeFileSync(join(source, 'package.json'), JSON.stringify({ scripts: { test: 'node --test' } }));
  execFileSync('git', ['-C', source, 'add', 'package.json']);
  execFileSync('git', ['-C', source, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-m', 'initial']);

  process.env.GAH_PROJECTS_ROOT = projectsRoot;
  process.env.XDG_DATA_HOME = join(root, 'data');
  const remoteUrl = 'https://github.com/owner/repo.git';
  const git = async (args: string[]): Promise<string> => {
    const cloneUrlIndex = args.indexOf(remoteUrl);
    const effectiveArgs = [...args];
    if (cloneUrlIndex >= 0) effectiveArgs[cloneUrlIndex] = source;
    const output = execFileSync('git', effectiveArgs, { encoding: 'utf8' }).trim();
    if (cloneUrlIndex >= 0) {
      execFileSync('git', ['-C', args.at(-1)!, 'remote', 'set-url', 'origin', remoteUrl]);
    }
    return output;
  };

  // The profile exists in config.toml but its checkout was never cloned on
  // this node (or was deleted) -- the common 'resurrect on a new node' case.
  let addCalls = 0;
  const configured: ProfileSummary[] = [{
    name: 'owner-repo',
    display_name: 'repo',
    provider: 'github',
    repo: 'owner/repo',
    repo_id: 'owner-repo',
    local_path: missingCheckout,
    worktree_base: '/worktrees',
    web_url: remoteUrl.replace(/\.git$/, ''),
    max_parallel_workers: null,
    max_open_managed_mrs: 1,
    manager_wake_autonomy: null,
    validation_timeout_seconds: 300
  }];
  const dependencies = {
    listProfiles: async () => configured,
    addProfile: async () => { addCalls += 1; },
    git
  };

  const imported = await importGitProject({ gitUrl: remoteUrl }, dependencies);
  assert.equal(imported.checkoutStatus, 'cloned');
  assert.equal(imported.checkoutPath, missingCheckout);
  assert.equal(existsSync(join(missingCheckout, '.git')), true);
  assert.equal(addCalls, 0, 'existing profile must not be re-added');
});


test('catalog migrates local strings, preserves remote ownership while offline, and removes only the selected entry', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-project-migration-'));
  const path = join(root, 'projects.json');
  process.env.GAH_PROJECT_CATALOG_PATH = path;
  writeFileSync(path, JSON.stringify({ profiles: ['same', 'same'] }));
  assert.deepEqual(listProjects([profile('same')], 'local').map(({ node_id, chat_profile }) => ({ node_id, chat_profile })), [{ node_id: 'local', chat_profile: 'same' }]);
  assert.equal(JSON.parse(readFileSync(path, 'utf8')).schema_version, 2);
  const checkout = join(root, 'checkout.txt');
  writeFileSync(checkout, 'owned by worker');
  const imported = addRemoteProject('worker:one', { ...profile('same'), local_path: checkout, unexpected_token: 'must-not-persist' }, 'local');
  assert.equal(imported.chat_profile, 'gah-node:worker%3Aone:same');
  assert.ok(!readFileSync(path, 'utf8').includes('must-not-persist'));
  assert.equal(listProjects([], 'local')[0].node_id, 'worker:one');
  const dependencies = { localNodeId: 'local', listProfiles: async () => [profile('same')] };
  assert.equal((await resolveChatProject(imported.chat_profile, dependencies))?.node_id, 'worker:one');
  await assert.rejects(resolveChatProject(imported.chat_profile, { ...dependencies, listProfiles: async () => [profile(imported.chat_profile)] }), /conflicts/);
  await assert.rejects(resolveChatProject('gah-node:worker%3aone:same', dependencies), /Invalid/);
  assert.equal(removeProject('same', 'worker:one', 'local'), true);
  assert.equal(existsSync(checkout), true);
  assert.deepEqual(listProjects([profile('same')], 'local').map((entry) => entry.node_id), ['local']);
});

test('catalog corruption retains the path and never silently migrates to an empty list', () => {
  const path = join(mkdtempSync(join(tmpdir(), 'gah-project-invalid-')), 'projects.json');
  process.env.GAH_PROJECT_CATALOG_PATH = path;
  for (const content of ['{bad json', '{"profiles":[3]}', '{"schema_version":2,"projects":[{"node_id":"remote","profile":"missing-summary"}]}']) {
    writeFileSync(path, content);
    assert.throws(() => listProjects([], 'local'), (error: unknown) => error instanceof Error && error.message.includes(path));
    assert.equal(readFileSync(path, 'utf8'), content);
  }
});

test('custom GitLab import requires explicit provider and preserves API authority without credential URLs', async () => {
  const url = 'https://git.example.test:8443/team/sub/repo.git';
  assert.throws(() => parseGitUrl(url), /Select GitLab/);
  assert.equal(parseGitUrl(url, { provider: 'gitlab' }).providerApiBase, 'https://git.example.test:8443/api/v4');
  assert.equal(parseGitUrl('ssh://git@git.example.test:2222/team/repo', { provider: 'gitlab' }).providerApiBase, 'https://git.example.test/api/v4');
  assert.throws(() => parseGitUrl(url, { provider: 'gitlab', providerApiBase: 'https://user:secret@git.example.test/api/v4' }), /credentials/);
  assert.throws(() => parseGitUrl(url, { provider: 'gitlab', providerApiBase: 'https://elsewhere.test/api/v4' }), /repository host/);
  assert.throws(() => parseGitUrl('https://github.com/a/b', { provider: 'gitlab' }), /does not match/);
  let touched = false;
  await assert.rejects(importGitProject({ gitUrl: url, provider: 'gitlab' }, { listProfiles: async () => { touched = true; return []; }, addProfile: async () => {} }), /numeric project ID/);
  assert.equal(touched, false);
});

test('GitLab import stores API base and project ID, distinguishes hosts, and repairs verified profile metadata', async () => {
  const root = mkdtempSync(join(tmpdir(), 'gah-gitlab-import-'));
  process.env.GAH_PROJECTS_ROOT = root;
  const gitUrl = 'https://git.example.test:8443/owner/repo.git';
  const input = { gitUrl, provider: 'gitlab' as const, providerProjectId: '123' };
  const configured = [{ ...profile('owner-repo'), provider: 'gitlab', repo: 'owner/repo', web_url: 'https://other.example.test/owner/repo' }];
  let added: ProfileAddOptions | undefined;
  let repaired: unknown;
  const dependencies = {
    listProfiles: async () => configured,
    addProfile: async (options: ProfileAddOptions) => { added = options; },
    setProfile: async (options: unknown) => { repaired = options; },
    git: async (args: string[]) => {
      if (args[0] === 'clone') { mkdirSync(args.at(-1)!, { recursive: true }); return ''; }
      if (args.includes('--is-inside-work-tree')) return 'true';
      if (args.includes('get-url')) return gitUrl;
      if (args.includes('symbolic-ref')) return 'main';
      return '';
    }
  };
  const imported = await importGitProject(input, dependencies);
  assert.equal(imported.profileName, 'owner-repo-2', 'same path on another GitLab host is a different project');
  assert.equal(added?.provider_api_base, 'https://git.example.test:8443/api/v4');
  assert.equal(added?.provider_project_id, '123');
  configured.push({ ...profile(imported.profileName), provider: 'gitlab', repo: 'owner/repo', local_path: imported.checkoutPath, web_url: 'https://git.example.test:8443/owner/repo' });
  assert.equal((await importGitProject(input, dependencies)).checkoutStatus, 'verified');
  assert.deepEqual(repaired, { name: imported.profileName, provider_api_base: 'https://git.example.test:8443/api/v4', provider_project_id: '123' });
});
