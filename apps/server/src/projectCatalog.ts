import {
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync
} from 'node:fs';
import { homedir } from 'node:os';
import { spawn } from 'node:child_process';
import { dirname, parse, relative, resolve } from 'node:path';
import type { ProfileSummary, ProjectImportData } from '@git-agent-harness/contracts';
import { runProfileAdd, runProfileList, type ProfileAddOptions } from './gahCli.js';

interface ProjectCatalogFile {
  profiles: string[];
}

function catalogPath(): string {
  return process.env.GAH_PROJECT_CATALOG_PATH
    || resolve(process.env.XDG_CONFIG_HOME || resolve(homedir(), '.config'), 'gah/projects.json');
}

function readCatalog(): ProjectCatalogFile {
  const path = catalogPath();
  if (!existsSync(path)) return { profiles: [] };
  const parsed = JSON.parse(readFileSync(path, 'utf8')) as unknown;
  if (
    typeof parsed !== 'object'
    || parsed === null
    || !Array.isArray((parsed as { profiles?: unknown }).profiles)
    || !(parsed as { profiles: unknown[] }).profiles.every((profile) => typeof profile === 'string')
  ) {
    throw new Error(`Invalid project catalog at ${path}`);
  }
  return { profiles: [...new Set((parsed as ProjectCatalogFile).profiles)] };
}

function writeCatalog(catalog: ProjectCatalogFile): void {
  const path = catalogPath();
  mkdirSync(dirname(path), { recursive: true });
  const temporaryPath = `${path}.${process.pid}.tmp`;
  writeFileSync(temporaryPath, `${JSON.stringify(catalog, null, 2)}\n`);
  renameSync(temporaryPath, path);
}

export function listProjects(profiles: ProfileSummary[]): ProfileSummary[] {
  const byName = new Map(profiles.map((profile) => [profile.name, profile]));
  return readCatalog().profiles.flatMap((name) => {
    const profile = byName.get(name);
    return profile ? [profile] : [];
  });
}

export function addProject(name: string, profiles: ProfileSummary[]): ProfileSummary {
  const profile = profiles.find((candidate) => candidate.name === name);
  if (!profile) throw new Error(`Profile '${name}' is not configured`);
  const catalog = readCatalog();
  if (!catalog.profiles.includes(name)) {
    catalog.profiles.push(name);
    writeCatalog(catalog);
  }
  return profile;
}

export function removeProject(name: string): boolean {
  const catalog = readCatalog();
  const profiles = catalog.profiles.filter((profile) => profile !== name);
  if (profiles.length === catalog.profiles.length) return false;
  writeCatalog({ profiles });
  return true;
}

type GitProvider = 'github' | 'gitlab';

export interface GitProjectIdentity {
  provider: GitProvider;
  host: string;
  repo: string;
  name: string;
  cloneUrl: string;
}

interface PreparedGitProject {
  profileName: string;
  checkoutPath: string;
  checkoutStatus: 'cloned' | 'verified' | 'recloned';
  detectedLanguages: string[];
  validationCommands: string[];
}

export function parseGitUrl(value: string): GitProjectIdentity {
  const cloneUrl = value.trim();
  let host = '';
  let repoPath = '';
  const scp = /^git@([^:]+):(.+)$/.exec(cloneUrl);
  if (scp) {
    host = scp[1].toLowerCase();
    repoPath = scp[2];
  } else {
    let url: URL;
    try {
      url = new URL(cloneUrl);
    } catch {
      throw new Error('Enter a full GitHub or GitLab HTTPS/SSH URL');
    }
    if (!['https:', 'ssh:'].includes(url.protocol)) {
      throw new Error('Enter a full GitHub or GitLab HTTPS/SSH URL');
    }
    if (url.password || (url.protocol === 'https:' && url.username)) {
      throw new Error('Git URLs must not contain credentials');
    }
    if (url.search || url.hash) {
      throw new Error('Git URLs must not contain query parameters or fragments');
    }
    if (url.protocol === 'ssh:' && url.username && url.username !== 'git') {
      throw new Error('SSH Git URLs must use the git user');
    }
    host = url.hostname.toLowerCase();
    repoPath = url.pathname.replace(/^\//, '');
  }

  if (host !== 'github.com' && host !== 'gitlab.com') {
    throw new Error('Only GitHub or GitLab hosted repositories are supported');
  }
  const provider: GitProvider = host === 'github.com' ? 'github' : 'gitlab';
  const repo = repoPath.replace(/\.git$/, '').replace(/\/$/, '');
  const parts = repo.split('/');
  if (parts.length < 2 || parts.some((part) => !/^[A-Za-z0-9._-]+$/.test(part) || part === '.' || part === '..')) {
    throw new Error('Git URL must include a valid owner and repository');
  }
  return { provider, host, repo: parts.join('/'), name: parts.at(-1)!, cloneUrl };
}

function dataRoot(): string {
  return process.env.XDG_DATA_HOME || resolve(homedir(), '.local/share');
}

function projectsRoot(): string {
  const root = resolve(process.env.GAH_PROJECTS_ROOT || resolve(dataRoot(), 'gah/projects'));
  if (root === parse(root).root) throw new Error('Managed projects root cannot be the filesystem root');
  return root;
}

function profileName(identity: GitProjectIdentity, profiles: ProfileSummary[]): string {
  const base = identity.repo.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  const conflict = profiles.find((profile) => profile.name === base);
  if (!conflict || (conflict.provider === identity.provider && conflict.repo === identity.repo)) return base;
  let suffix = 2;
  while (profiles.some((profile) => profile.name === `${base}-${suffix}`)) suffix += 1;
  return `${base}-${suffix}`;
}

function runGit(args: string[]): Promise<string> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn('git', args, {
      env: { ...process.env, GIT_TERMINAL_PROMPT: '0' },
      stdio: ['ignore', 'pipe', 'pipe']
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (data) => { stdout += data.toString(); });
    child.stderr.on('data', (data) => { stderr += data.toString(); });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolvePromise(stdout.trim());
      else reject(new Error(stderr.trim() || `git exited with status ${code}`));
    });
  });
}

async function verifyCheckout(
  path: string,
  expected: GitProjectIdentity,
  git: (args: string[]) => Promise<string> = runGit
): Promise<void> {
  if (lstatSync(path).isSymbolicLink()) throw new Error('Managed checkout cannot be a symbolic link');
  if (await git(['-C', path, 'rev-parse', '--is-inside-work-tree']) !== 'true') {
    throw new Error('Managed checkout is not a Git worktree');
  }
  const origin = parseGitUrl(await git(['-C', path, 'remote', 'get-url', 'origin']));
  if (origin.provider !== expected.provider || origin.repo !== expected.repo) {
    throw new Error(`Managed checkout origin is ${origin.repo}, expected ${expected.repo}`);
  }
  if (await git(['-C', path, 'status', '--porcelain'])) {
    throw new Error(`Managed checkout at ${path} has uncommitted changes`);
  }
}

function detectedValidation(path: string): { languages: string[]; commands: string[] } {
  const languages: string[] = [];
  const commands: string[] = [];
  if (existsSync(resolve(path, 'package.json'))) {
    languages.push('JavaScript/TypeScript');
    try {
      const pkg = JSON.parse(readFileSync(resolve(path, 'package.json'), 'utf8')) as { scripts?: Record<string, unknown> };
      const manager = existsSync(resolve(path, 'pnpm-lock.yaml')) ? 'pnpm' : existsSync(resolve(path, 'yarn.lock')) ? 'yarn' : 'npm';
      for (const script of ['test', 'typecheck', 'lint']) {
        if (typeof pkg.scripts?.[script] === 'string') {
          commands.push(manager === 'npm' ? (script === 'test' ? 'npm test' : `npm run ${script}`) : `${manager} ${script}`);
        }
      }
    } catch {
      // A malformed package manifest should not prevent importing the checkout.
    }
  }
  if (existsSync(resolve(path, 'Cargo.toml'))) {
    languages.push('Rust');
    commands.push('cargo test');
  }
  if (existsSync(resolve(path, 'go.mod'))) {
    languages.push('Go');
    commands.push('go test ./...');
  }
  if (existsSync(resolve(path, 'pyproject.toml')) || existsSync(resolve(path, 'requirements.txt'))) {
    languages.push('Python');
    commands.push('python -m pytest');
  }
  return { languages, commands: [...new Set(commands)] };
}

export async function importGitProject(
  input: ProjectImportData,
  dependencies: {
    listProfiles: typeof runProfileList;
    addProfile: typeof runProfileAdd;
    git?: (args: string[]) => Promise<string>;
  } = { listProfiles: runProfileList, addProfile: runProfileAdd }
): Promise<PreparedGitProject> {
  const git = dependencies.git ?? runGit;
  const identity = parseGitUrl(input.gitUrl);
  const profiles = await dependencies.listProfiles();
  const existing = profiles.find((profile) => profile.provider === identity.provider && profile.repo === identity.repo);
  // A configured profile whose checkout exists is verified in place. If the
  // checkout is missing (profile in config.toml, repo never cloned on this
  // node -- the common 'resurrect on a new node' case), fall through to the
  // clone path below instead of hard-failing on an lstat ENOENT.
  if (existing && !input.reclone && existsSync(existing.local_path)) {
    await verifyCheckout(existing.local_path, identity, git);
    const detected = detectedValidation(existing.local_path);
    return {
      profileName: existing.name,
      checkoutPath: existing.local_path,
      checkoutStatus: 'verified',
      detectedLanguages: detected.languages,
      validationCommands: detected.commands
    };
  }

  const name = existing?.name ?? profileName(identity, profiles);
  const root = projectsRoot();
  const checkoutPath = resolve(existing?.local_path ?? resolve(root, name));
  const checkoutRelativePath = relative(root, checkoutPath);
  if (checkoutRelativePath === '' || checkoutRelativePath.startsWith('..')) {
    // Overwriting an EXISTING checkout outside the managed root is never
    // allowed -- the path is not request-controlled (it comes from the
    // operator's own config.toml), but a path that already exists could be a
    // live checkout with history we must not clobber. A configured local_path
    // that is MISSING is safe to clone into: there is nothing there to lose.
    if (existsSync(checkoutPath) || !existing) {
      throw new Error('Only managed checkouts can be re-cloned');
    }
  }
  mkdirSync(dirname(checkoutPath), { recursive: true });
  mkdirSync(root, { recursive: true });

  const existed = existsSync(checkoutPath);
  let backupPath: string | undefined;
  let checkoutStatus: PreparedGitProject['checkoutStatus'] = 'cloned';
  try {
    if (existed) {
      await verifyCheckout(checkoutPath, identity, git);
      if (!input.reclone) {
        checkoutStatus = 'verified';
      } else {
        backupPath = `${checkoutPath}.backup-${process.pid}-${Date.now()}`;
        renameSync(checkoutPath, backupPath);
        await git(['clone', '--origin', 'origin', '--', identity.cloneUrl, checkoutPath]);
        await verifyCheckout(checkoutPath, identity, git);
        checkoutStatus = 'recloned';
      }
    } else {
      await git(['clone', '--origin', 'origin', '--', identity.cloneUrl, checkoutPath]);
      await verifyCheckout(checkoutPath, identity, git);
    }

    const detected = detectedValidation(checkoutPath);
    const defaultBranch = await git(['-C', checkoutPath, 'symbolic-ref', '--short', 'HEAD']);
    const options: ProfileAddOptions = {
      name,
      display_name: identity.name,
      repo_id: name,
      provider: identity.provider,
      repo: identity.repo,
      local_path: checkoutPath,
      artifact_root: resolve(dataRoot(), 'gah/artifacts', name),
      default_target_branch: defaultBranch,
      ...(identity.provider === 'gitlab' ? { provider_api_base: 'https://gitlab.com/api/v4' } : {}),
      ...(detected.commands.length > 0 ? { validation_commands: detected.commands } : {})
    };
    if (!existing) await dependencies.addProfile(options);
    if (backupPath) rmSync(backupPath, { recursive: true, force: true });
    return {
      profileName: name,
      checkoutPath,
      checkoutStatus,
      detectedLanguages: detected.languages,
      validationCommands: detected.commands
    };
  } catch (error) {
    if (backupPath) {
      rmSync(checkoutPath, { recursive: true, force: true });
      renameSync(backupPath, checkoutPath);
    } else if (!existed) {
      rmSync(checkoutPath, { recursive: true, force: true });
    }
    throw error;
  }
}
