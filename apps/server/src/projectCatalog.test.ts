import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, test } from 'node:test';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import type { ProfileAddOptions } from './gahCli.js';
import { addProject, importGitProject, listProjects, parseGitUrl, removeProject } from './projectCatalog.js';

const savedCatalogPath = process.env.GAH_PROJECT_CATALOG_PATH;
const savedEnvironment = new Map(
  ['GAH_PROJECTS_ROOT', 'XDG_DATA_HOME']
    .map((name) => [name, process.env[name]])
);

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

