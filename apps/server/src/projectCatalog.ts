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
import type { ProfileSummary, ProjectImportData, ProjectSummary } from '@git-agent-harness/contracts';
import { projectChatProfile } from '@git-agent-harness/contracts';
import { getCoordinatorIdentity } from './coordinatorIdentity.js';
import { runProfileAdd, runProfileList, runProfileSet, type ProfileAddOptions } from './gahCli.js';

interface ProjectCatalogEntry {
  node_id: string;
  profile: string;
  /** Remote metadata stays available when its worker is offline. Local profiles remain authoritative. */
  summary?: ProfileSummary;
}

interface ProjectCatalogFile {
  schema_version: 2;
  projects: ProjectCatalogEntry[];
}

function catalogPath(): string {
  return process.env.GAH_PROJECT_CATALOG_PATH
    || resolve(process.env.XDG_CONFIG_HOME || resolve(homedir(), '.config'), 'gah/projects.json');
}

/** Validate and project worker metadata; unknown fields, including credentials, never enter the catalog. */
export function projectProfile(value: unknown): ProfileSummary {
  if (!value || typeof value !== 'object') throw new Error('Invalid project profile');
  const p = value as ProfileSummary;
  for (const key of ['name', 'provider', 'repo', 'repo_id', 'local_path', 'worktree_base'] as const) {
    if (typeof p[key] !== 'string' || !p[key].trim()) throw new Error(`Invalid project profile ${key}`);
  }
  if ((typeof p.display_name !== 'string')
    || (p.web_url !== null && typeof p.web_url !== 'string')
    || (p.max_parallel_workers !== null && !Number.isFinite(p.max_parallel_workers))
    || !Number.isFinite(p.max_open_managed_mrs) || !Number.isFinite(p.validation_timeout_seconds)
    || (p.manager_wake_autonomy !== null && !['off', 'review_only', 'full'].includes(p.manager_wake_autonomy))) {
    throw new Error('Invalid project profile metadata');
  }
  if (p.web_url) {
    const url = new URL(p.web_url);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Invalid project web URL');
  }
  return {
    name: p.name, display_name: p.display_name, provider: p.provider, repo: p.repo,
    repo_id: p.repo_id, local_path: p.local_path, worktree_base: p.worktree_base,
    web_url: p.web_url, max_parallel_workers: p.max_parallel_workers,
    max_open_managed_mrs: p.max_open_managed_mrs, manager_wake_autonomy: p.manager_wake_autonomy,
    validation_timeout_seconds: p.validation_timeout_seconds,
    ...(p.delivery_mode === 'pr' || p.delivery_mode === 'handoff' ? { delivery_mode: p.delivery_mode } : {}),
    ...(Number.isFinite(p.chat_session_idle_days) ? { chat_session_idle_days: p.chat_session_idle_days } : {})
  };
}

function readCatalog(localNodeId: string): ProjectCatalogFile {
  const path = catalogPath();
  if (!existsSync(path)) return { schema_version: 2, projects: [] };
  try {
    const parsed = JSON.parse(readFileSync(path, 'utf8'));
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('Expected an object');
    if (parsed.schema_version === undefined && Array.isArray(parsed.profiles)
      && parsed.profiles.every((profile: unknown) => typeof profile === 'string' && profile.trim())) {
      const migrated: ProjectCatalogFile = { schema_version: 2, projects: [...new Set<string>(parsed.profiles)].map((profile) => ({ node_id: localNodeId, profile })) };
      writeCatalog(migrated);
      return migrated;
    }
    if (parsed.schema_version !== 2 || !Array.isArray(parsed.projects)) throw new Error('Expected catalog schema version 2');
    const seen = new Set<string>();
    const projects = parsed.projects.map((entry: ProjectCatalogEntry) => {
      if (!entry || typeof entry.node_id !== 'string' || !entry.node_id.trim()
        || typeof entry.profile !== 'string' || !entry.profile.trim()) throw new Error('Invalid project owner or profile');
      const key = JSON.stringify([entry.node_id, entry.profile]);
      if (seen.has(key)) throw new Error('Duplicate project owner and profile');
      seen.add(key);
      const summary = entry.summary === undefined ? undefined : projectProfile(entry.summary);
      if (summary && summary.name !== entry.profile) throw new Error('Project summary does not match its profile');
      if (entry.node_id !== localNodeId && !summary) throw new Error('Remote project summary is missing');
      return { node_id: entry.node_id, profile: entry.profile, ...(summary ? { summary } : {}) };
    });
    return { schema_version: 2, projects };
  } catch (error) {
    throw new Error(`Invalid project catalog at ${path}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function writeCatalog(catalog: ProjectCatalogFile): void {
  const path = catalogPath();
  mkdirSync(dirname(path), { recursive: true });
  const temporaryPath = `${path}.${process.pid}.tmp`;
  writeFileSync(temporaryPath, `${JSON.stringify(catalog, null, 2)}\n`);
  renameSync(temporaryPath, path);
}

export function listProjects(profiles: ProfileSummary[], localNodeId = getCoordinatorIdentity().node_id): ProjectSummary[] {
  const byName = new Map(profiles.map((profile) => [profile.name, profile]));
  return readCatalog(localNodeId).projects.flatMap((entry) => {
    const profile = entry.node_id === localNodeId ? byName.get(entry.profile) : entry.summary;
    return profile ? [{ ...profile, node_id: entry.node_id, chat_profile: projectChatProfile(profile.name, entry.node_id, localNodeId) }] : [];
  });
}

/** Central's remote projection does not materialize a local profile or touch a checkout. */
export function addRemoteProject(nodeId: string, value: unknown, localNodeId = getCoordinatorIdentity().node_id): ProjectSummary {
  if (!nodeId.trim() || nodeId === localNodeId) throw new Error('A remote project requires a worker node');
  const summary = projectProfile(value);
  const catalog = readCatalog(localNodeId);
  const entry = { node_id: nodeId, profile: summary.name, summary };
  const index = catalog.projects.findIndex((candidate) => candidate.node_id === nodeId && candidate.profile === summary.name);
  if (index < 0) catalog.projects.push(entry);
  else catalog.projects[index] = entry;
  writeCatalog(catalog);
  return { ...summary, node_id: nodeId, chat_profile: projectChatProfile(summary.name, nodeId, localNodeId) };
}

export function addProject(name: string, profiles: ProfileSummary[], localNodeId = getCoordinatorIdentity().node_id): ProjectSummary {
  const profile = profiles.find((candidate) => candidate.name === name);
  if (!profile) throw new Error(`Profile '${name}' is not configured`);
  const catalog = readCatalog(localNodeId);
  if (!catalog.projects.some((entry) => entry.node_id === localNodeId && entry.profile === name)) {
    catalog.projects.push({ node_id: localNodeId, profile: name });
    writeCatalog(catalog);
  }
  return { ...profile, node_id: localNodeId, chat_profile: profile.name };
}

export function removeProject(name: string, nodeId = getCoordinatorIdentity().node_id, localNodeId = getCoordinatorIdentity().node_id): boolean {
  const catalog = readCatalog(localNodeId);
  const projects = catalog.projects.filter((entry) => entry.node_id !== nodeId || entry.profile !== name);
  if (projects.length === catalog.projects.length) return false;
  writeCatalog({ schema_version: 2, projects });
  return true;
}

/** Resolve a catalog selection without creating a central profile for a worker checkout. */
export async function getProject(nodeId: string, profileName: string): Promise<ProjectSummary | undefined> {
  const localNodeId = getCoordinatorIdentity().node_id;
  const profiles = nodeId === localNodeId ? await runProfileList() : [];
  return listProjects(profiles, localNodeId).find((project) => project.node_id === nodeId && project.name === profileName);
}

/** Decode a remote conversation identity only when its owner exists in the central catalog. */
export async function resolveChatProject(chatProfile: string, dependencies = { localNodeId: getCoordinatorIdentity().node_id, listProfiles: runProfileList }): Promise<ProjectSummary | undefined> {
  const { localNodeId } = dependencies;
  const profiles = await dependencies.listProfiles();
  const local = profiles.find((profile) => profile.name === chatProfile);
  if (!chatProfile.startsWith('gah-node:')) return local ? { ...local, node_id: localNodeId, chat_profile: local.name } : undefined;
  if (local) throw new Error('Conversation identity conflicts with a configured local profile. Rename that local profile.');
  const parts = chatProfile.split(':');
  if (parts.length !== 3) throw new Error('Invalid remote project conversation identity');
  let nodeId: string;
  let profileName: string;
  try { nodeId = decodeURIComponent(parts[1]); profileName = decodeURIComponent(parts[2]); }
  catch { throw new Error('Invalid remote project conversation identity'); }
  if (nodeId === localNodeId || !nodeId || !profileName || projectChatProfile(profileName, nodeId, localNodeId) !== chatProfile) throw new Error('Invalid remote project conversation identity');
  return listProjects(profiles, localNodeId).find((project) => project.node_id === nodeId && project.name === profileName);
}

type GitProvider = 'github' | 'gitlab';

export interface GitProjectIdentity {
  provider: GitProvider;
  host: string;
  repo: string;
  name: string;
  cloneUrl: string;
  providerApiBase?: string;
}

interface PreparedGitProject {
  profileName: string;
  checkoutPath: string;
  checkoutStatus: 'cloned' | 'verified' | 'recloned';
  detectedLanguages: string[];
  validationCommands: string[];
}

export function parseGitUrl(value: string, options: Pick<ProjectImportData, 'provider' | 'providerApiBase'> = {}): GitProjectIdentity {
  const cloneUrl = value.trim();
  let host = '';
  let repoPath = '';
  let httpsOrigin: string | undefined;
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
    if (url.protocol === 'https:') httpsOrigin = url.origin;
    repoPath = url.pathname.replace(/^\//, '');
  }

  if (options.provider !== undefined && !['github', 'gitlab'].includes(options.provider)) throw new Error('Select GitHub or GitLab as the repository provider');
  const hostedProvider = host === 'github.com' ? 'github' : host === 'gitlab.com' ? 'gitlab' : undefined;
  if (!hostedProvider && options.provider !== 'gitlab') throw new Error('Select GitLab explicitly for a custom GitLab host');
  if (hostedProvider && options.provider && options.provider !== hostedProvider) throw new Error('Repository host does not match the selected provider');
  const provider: GitProvider = hostedProvider ?? 'gitlab';
  if (!/^[a-z0-9.-]+$/.test(host) || host.startsWith('.') || host.endsWith('.')) throw new Error('Invalid Git repository host');
  let providerApiBase: string | undefined;
  if (provider === 'gitlab') {
    const api = new URL(options.providerApiBase || `${httpsOrigin || `https://${host}`}/api/v4`);
    if (api.protocol !== 'https:' || api.username || api.password || api.search || api.hash
      || api.hostname.toLowerCase() !== host || api.pathname.replace(/\/$/, '') !== '/api/v4') {
      throw new Error('GitLab API URL must use HTTPS on the repository host, use the root /api/v4 path, and contain no credentials, query, or fragment');
    }
    providerApiBase = api.toString().replace(/\/$/, '');
  } else if (options.providerApiBase) throw new Error('GitLab API URL applies only to GitLab repositories');
  const repo = repoPath.replace(/\.git$/, '').replace(/\/$/, '');
  const parts = repo.split('/');
  if (parts.length < 2 || parts.some((part) => !/^[A-Za-z0-9._-]+$/.test(part) || part === '.' || part === '..')) {
    throw new Error('Git URL must include a valid owner and repository');
  }
  return { provider, host, repo: parts.join('/'), name: parts.at(-1)!, cloneUrl, ...(providerApiBase ? { providerApiBase } : {}) };
}

function dataRoot(): string {
  return process.env.XDG_DATA_HOME || resolve(homedir(), '.local/share');
}

function projectsRoot(): string {
  const root = resolve(process.env.GAH_PROJECTS_ROOT || resolve(dataRoot(), 'gah/projects'));
  if (root === parse(root).root) throw new Error('Managed projects root cannot be the filesystem root');
  return root;
}

function matchesProject(profile: ProfileSummary, identity: GitProjectIdentity): boolean {
  if (profile.provider !== identity.provider || profile.repo !== identity.repo) return false;
  if (!profile.web_url) return identity.host === 'github.com' || identity.host === 'gitlab.com';
  try { return identity.providerApiBase ? new URL(profile.web_url).origin === new URL(identity.providerApiBase).origin : new URL(profile.web_url).hostname.toLowerCase() === identity.host; }
  catch { return false; }
}

function profileName(identity: GitProjectIdentity, profiles: ProfileSummary[]): string {
  const base = identity.repo.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  const conflict = profiles.find((profile) => profile.name === base);
  if (!conflict || matchesProject(conflict, identity)) return base;
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
  const origin = parseGitUrl(await git(['-C', path, 'remote', 'get-url', 'origin']), { provider: expected.provider, providerApiBase: expected.providerApiBase });
  if (origin.provider !== expected.provider || origin.host !== expected.host || origin.repo !== expected.repo
    || (origin.cloneUrl.startsWith('https:') && expected.cloneUrl.startsWith('https:') && new URL(origin.cloneUrl).origin !== new URL(expected.cloneUrl).origin)) {
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
    setProfile?: typeof runProfileSet;
    git?: (args: string[]) => Promise<string>;
  } = { listProfiles: runProfileList, addProfile: runProfileAdd }
): Promise<PreparedGitProject> {
  const git = dependencies.git ?? runGit;
  const identity = parseGitUrl(input.gitUrl, input);
  if (identity.provider === 'gitlab' && !/^[1-9][0-9]*$/.test(input.providerProjectId || '')) {
    throw new Error('GitLab imports require the numeric project ID from the project overview');
  }
  const profiles = await dependencies.listProfiles();
  const existing = profiles.find((profile) => matchesProject(profile, identity));
  // A configured profile whose checkout exists is verified in place. If the
  // checkout is missing (profile in config.toml, repo never cloned on this
  // node -- the common 'resurrect on a new node' case), fall through to the
  // clone path below instead of hard-failing on an lstat ENOENT.
  if (existing && !input.reclone && existsSync(existing.local_path)) {
    await verifyCheckout(existing.local_path, identity, git);
    if (identity.provider === 'gitlab') await (dependencies.setProfile ?? runProfileSet)({ name: existing.name, provider_api_base: identity.providerApiBase, provider_project_id: input.providerProjectId });
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
      ...(identity.provider === 'gitlab' ? { provider_api_base: identity.providerApiBase, provider_project_id: input.providerProjectId } : {}),
      ...(detected.commands.length > 0 ? { validation_commands: detected.commands } : {})
    };
    if (!existing) await dependencies.addProfile(options);
    else if (identity.provider === 'gitlab') await (dependencies.setProfile ?? runProfileSet)({ name, provider_api_base: identity.providerApiBase, provider_project_id: input.providerProjectId });
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
