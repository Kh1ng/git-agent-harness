/**
 * In-app "Update GAH" flow (issue #989). Reuses the existing `gah update
 * --role central --restart-server` command for every actual step (git pull,
 * npm ci/build, systemd unit refresh, service restart) -- this module only
 * launches that command and persists its progress to disk so an HTTP client
 * can poll it across the restart it triggers.
 *
 * State lives on disk, not in memory: `gah update --restart-server` ends by
 * restarting this very process (`sudo systemctl restart gah-server.service`),
 * which would wipe any in-memory tracking before a client could observe the
 * final result. Writing to a file that both the old and the freshly
 * restarted process read makes status/output reconnect-safe by construction
 * -- whichever process answers the next GET, the answer is the same.
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { homedir } from 'node:os';
import type { AdminUpdateCommitInfo, AdminUpdatePendingInfo, AdminUpdateState } from '@git-agent-harness/contracts';
import { findGahBinary } from './gahCli.js';

export type { AdminUpdateCommitInfo, AdminUpdatePendingInfo, AdminUpdateStatus, AdminUpdateState } from '@git-agent-harness/contracts';

const IDLE_STATE: AdminUpdateState = {
  status: 'idle',
  startedAt: null,
  finishedAt: null,
  exitCode: null,
  pid: null,
  output: ''
};

/** Bound the persisted output so a long build can't grow the state file
 * without limit; keeps the tail, which is what matters for diagnosing a
 * failure. */
const MAX_OUTPUT_CHARS = 200_000;

/** Outside the Git checkout by default: `gah update` refuses to run against
 * a dirty tree, so writing this file under `config/` (tracked, checkout-
 * relative) would make the update process dirty its own repo and fail its
 * own clean-tree guard on the very next run. */
function statePath(): string {
  return (
    process.env.GAH_ADMIN_UPDATE_STATE_PATH ||
    resolve(process.env.XDG_STATE_HOME || resolve(homedir(), '.local', 'state'), 'gah', 'admin-update-state.json')
  );
}

function parseState(data: unknown): AdminUpdateState {
  const record = data as Record<string, unknown>;
  const status = record.status;
  return {
    status:
      status === 'running' || status === 'success' || status === 'failed' || status === 'inferred_restart'
        ? status
        : 'idle',
    startedAt: typeof record.startedAt === 'string' ? record.startedAt : null,
    finishedAt: typeof record.finishedAt === 'string' ? record.finishedAt : null,
    exitCode: typeof record.exitCode === 'number' ? record.exitCode : null,
    pid: typeof record.pid === 'number' ? record.pid : null,
    output: typeof record.output === 'string' ? record.output : ''
  };
}

/** `gah update --restart-server`'s final step restarts this very server
 * (`systemctl restart gah-server.service`), which kills the whole cgroup --
 * including the updater child -- before its `close` handler can run and
 * record success. So the freshly restarted process that answers the next
 * read must reconcile a `running` state whose pid is already dead: the
 * fact that this process is up and answering IS evidence the restart step
 * was reached, but the exit code was never observed, so this lands on the
 * explicit `inferred_restart` state rather than being reported as `success`
 * or left stuck at `running` forever. A command failure *before* the
 * restart step is unaffected -- that updater process is still alive and its
 * own `close` handler writes `failed` directly. */
function reconcileDeadUpdater(state: AdminUpdateState): AdminUpdateState {
  if (state.status !== 'running' || state.pid === null || pidAlive(state.pid)) return state;
  const reconciled: AdminUpdateState = {
    ...state,
    status: 'inferred_restart',
    finishedAt: new Date().toISOString()
  };
  writeAdminUpdateState(reconciled);
  return reconciled;
}

export function readAdminUpdateState(): AdminUpdateState {
  const path = statePath();
  if (!existsSync(path)) return IDLE_STATE;
  try {
    const state = parseState(JSON.parse(readFileSync(path, 'utf8')));
    return reconcileDeadUpdater(state);
  } catch {
    return IDLE_STATE;
  }
}

/** Writes via a same-directory temp file + rename so a concurrent GET never
 * observes a partially-written file: `writeFileSync` on the live path
 * truncates in place, and a read racing that truncation sees invalid JSON,
 * falls back to `idle`, and a polling client stops and never reloads.
 * `rename(2)` within one directory is atomic. Mode 0600 keeps the build
 * output (which can include repo paths/command args) readable only by the
 * server's own user. */
function writeAdminUpdateState(state: AdminUpdateState): void {
  const path = statePath();
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  const tmpPath = `${path}.${process.pid}.tmp`;
  writeFileSync(tmpPath, JSON.stringify(state, null, 2), { mode: 0o600 });
  renameSync(tmpPath, path);
}

function pidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export interface StartAdminUpdateOptions {
  spawnFn?: typeof spawn;
}

export interface StartAdminUpdateResult {
  started: boolean;
  state: AdminUpdateState;
}

export function adminUpdateEnvironment(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  uid: number | undefined
): NodeJS.ProcessEnv {
  if (platform !== 'linux' || uid === undefined) return environment;
  const runtimeDir = `/run/user/${uid}`;
  return {
    ...environment,
    XDG_RUNTIME_DIR: environment.XDG_RUNTIME_DIR ?? runtimeDir,
    DBUS_SESSION_BUS_ADDRESS: environment.DBUS_SESSION_BUS_ADDRESS ?? `unix:path=${runtimeDir}/bus`
  };
}

/** Launches `gah update --repo <cwd> --role central --restart-server`
 * detached (so it outlives this HTTP request, and survives this process
 * being the one that gets restarted) and streams its combined output into
 * the state file. `--repo` pins it to this server's own checkout
 * (`process.cwd()`, i.e. `gah-server.service`'s `WorkingDirectory`) rather
 * than whatever `gah update` would otherwise infer. */
export function startAdminUpdate(options: StartAdminUpdateOptions = {}): StartAdminUpdateResult {
  const existing = readAdminUpdateState();
  if (existing.status === 'running' && existing.pid !== null && pidAlive(existing.pid)) {
    return { started: false, state: existing };
  }

  const spawnFn = options.spawnFn ?? spawn;
  const uid = process.platform === 'linux' && typeof process.getuid === 'function' ? process.getuid() : undefined;
  const child: ChildProcess = spawnFn(
    findGahBinary(),
    ['update', '--repo', process.cwd(), '--role', 'central', '--restart-server'],
    {
      cwd: process.cwd(),
      env: adminUpdateEnvironment(process.env, process.platform, uid),
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe']
    }
  );
  child.unref();

  let state: AdminUpdateState = {
    status: 'running',
    startedAt: new Date().toISOString(),
    finishedAt: null,
    exitCode: null,
    pid: child.pid ?? null,
    output: ''
  };
  writeAdminUpdateState(state);

  const appendOutput = (chunk: Buffer | string) => {
    state = { ...state, output: (state.output + chunk.toString()).slice(-MAX_OUTPUT_CHARS) };
    writeAdminUpdateState(state);
  };
  child.stdout?.on('data', appendOutput);
  child.stderr?.on('data', appendOutput);

  child.on('close', (code) => {
    state = {
      ...state,
      status: code === 0 ? 'success' : 'failed',
      finishedAt: new Date().toISOString(),
      exitCode: code
    };
    writeAdminUpdateState(state);
  });

  child.on('error', (error) => {
    state = {
      ...state,
      status: 'failed',
      finishedAt: new Date().toISOString(),
      output: `${state.output}\n[gah update] failed to spawn: ${error.message}`.slice(-MAX_OUTPUT_CHARS)
    };
    writeAdminUpdateState(state);
  });

  return { started: true, state };
}

export interface GetPendingCommitsOptions {
  gitFn?: typeof spawnSync;
}

function commitInfo(gitFn: typeof spawnSync, cwd: string, ref: string): AdminUpdateCommitInfo | null {
  const result = gitFn('git', ['log', '-1', `--pretty=format:%H|%h|%s`, ref], { cwd, encoding: 'utf8' });
  if (result.status !== 0 || !result.stdout) return null;
  const [hash, short, ...rest] = result.stdout.trim().split('|');
  if (!hash || !short) return null;
  return { hash, short, subject: rest.join('|') };
}

/** Read-only "what's pending" check: current HEAD vs the last-fetched
 * origin/main, same as /api/git/log this does not itself run `git fetch`. */
export function getPendingCommits(cwd: string, options: GetPendingCommitsOptions = {}): AdminUpdatePendingInfo {
  const gitFn = options.gitFn ?? spawnSync;
  const current = commitInfo(gitFn, cwd, 'HEAD');
  const latest = commitInfo(gitFn, cwd, 'origin/main');
  let commitsBehind = 0;
  if (current && latest && current.hash !== latest.hash) {
    const result = gitFn('git', ['rev-list', '--count', 'HEAD..origin/main'], { cwd, encoding: 'utf8' });
    commitsBehind = result.status === 0 ? parseInt(result.stdout.trim(), 10) || 0 : 0;
  }
  return {
    current,
    latest,
    commitsBehind,
    upToDate: !!current && !!latest && current.hash === latest.hash
  };
}
