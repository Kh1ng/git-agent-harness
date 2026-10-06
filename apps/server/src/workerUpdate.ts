/**
 * Worker-side release update service (issue #1416). Central asks a worker
 * to update; the worker pulls the same release artifact central installs
 * (`gah update --from-release --role worker`) instead of rebuilding from
 * source. A worker that is mid-dispatch finishes its run first: `start`
 * arms the update and a drain poll launches it once the last active claim
 * clears, so a dispatch is never killed by its own node's update.
 *
 * State lives on disk like the central admin update (adminUpdate.ts): the
 * update may end by restarting this very node server (macOS LaunchAgent),
 * which would wipe any in-memory tracking before central could observe it.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { homedir } from 'node:os';
import type { WorkerUpdateStatus } from '@git-agent-harness/contracts';
import { COORDINATOR_VERSION } from '@git-agent-harness/contracts';
import { findGahBinary, runClaimsList } from './gahCli.js';

const MAX_OUTPUT_CHARS = 200_000;
const DRAIN_POLL_MS = 15_000;

interface WorkerUpdateRecord extends WorkerUpdateStatus {
  pid: number | null;
}

/** Outside the Git checkout, same reasoning as adminUpdate.ts. */
function statePath(): string {
  return (
    process.env.GAH_WORKER_UPDATE_STATE_PATH ||
    resolve(process.env.XDG_STATE_HOME || resolve(homedir(), '.local', 'state'), 'gah', 'worker-update-state.json')
  );
}

function idleStatus(): WorkerUpdateStatus {
  return {
    status: 'idle',
    current_version: COORDINATOR_VERSION,
    target_version: null,
    armed_at: null,
    started_at: null,
    finished_at: null,
    active_dispatches: null,
    output: ''
  };
}

function pidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function writeState(record: WorkerUpdateRecord): void {
  const path = statePath();
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  const tmpPath = `${path}.${process.pid}.tmp`;
  writeFileSync(tmpPath, JSON.stringify(record, null, 2), { mode: 0o600 });
  renameSync(tmpPath, path);
}

function view(record: WorkerUpdateRecord): WorkerUpdateStatus {
  const { pid: _pid, ...status } = record;
  return status;
}

/** Live local claims, stale ones excluded: a stale claim is a dead dispatch,
 * not a run the update has to wait for. */
export async function countActiveClaims(): Promise<number> {
  const claims = await runClaimsList(undefined);
  return claims.filter((claim) => !claim.is_stale).length;
}

export interface WorkerUpdateDeps {
  spawnFn?: typeof spawn;
  /** Override for `gah claims list --json`; tests inject a stub. */
  countActiveClaims?: () => Promise<number>;
  /** Override for the post-success worker restart (macOS LaunchAgent);
   * tests inject a stub so nothing real is spawned. */
  restartWorker?: () => void;
  cwd?: string;
}

export class WorkerUpdateService {
  private drainTimer: NodeJS.Timeout | null = null;
  private readonly spawnFn: typeof spawn;
  private readonly cwd: string;

  constructor(private readonly deps: WorkerUpdateDeps = {}) {
    this.spawnFn = deps.spawnFn ?? spawn;
    this.cwd = deps.cwd ?? process.cwd();
  }

  private read(): WorkerUpdateRecord {
    const path = statePath();
    if (!existsSync(path)) return { ...idleStatus(), pid: null };
    try {
      const parsed = JSON.parse(readFileSync(path, 'utf8')) as Partial<WorkerUpdateRecord>;
      const base = idleStatus();
      const record: WorkerUpdateRecord = {
        status:
          parsed.status === 'waiting' || parsed.status === 'running' || parsed.status === 'success'
          || parsed.status === 'failed' || parsed.status === 'inferred_restart'
            ? parsed.status
            : 'idle',
        current_version: typeof parsed.current_version === 'string' ? parsed.current_version : base.current_version,
        target_version: typeof parsed.target_version === 'string' ? parsed.target_version : null,
        armed_at: typeof parsed.armed_at === 'string' ? parsed.armed_at : null,
        started_at: typeof parsed.started_at === 'string' ? parsed.started_at : null,
        finished_at: typeof parsed.finished_at === 'string' ? parsed.finished_at : null,
        active_dispatches:
          typeof parsed.active_dispatches === 'number' && Number.isFinite(parsed.active_dispatches)
            ? parsed.active_dispatches
            : null,
        output: typeof parsed.output === 'string' ? parsed.output : '',
        pid: typeof parsed.pid === 'number' ? parsed.pid : null
      };
      // A `running` record whose updater is gone recorded no exit code:
      // unlike central, a worker update never restarts this server as its
      // own final step (the macOS restart happens after success is
      // recorded), so a dead pid here means the updater was killed.
      if (record.status === 'running' && record.pid !== null && !pidAlive(record.pid)) {
        record.status = 'failed';
        record.finished_at = new Date().toISOString();
        record.output = `${record.output}\n[gah] the updater process disappeared without recording an exit`.slice(-MAX_OUTPUT_CHARS);
        writeState(record);
      }
      return record;
    } catch {
      return { ...idleStatus(), pid: null };
    }
  }

  status(): WorkerUpdateStatus {
    return view(this.read());
  }

  async start(): Promise<{ started: boolean; status: WorkerUpdateStatus }> {
    const existing = this.read();
    if (existing.status === 'running' && existing.pid !== null && pidAlive(existing.pid)) {
      return { started: false, status: view(existing) };
    }
    if (existing.status === 'waiting') {
      this.scheduleDrain();
      return { started: true, status: view(existing) };
    }
    const active = await this.countClaims();
    if (active > 0) {
      const armed: WorkerUpdateRecord = {
        ...idleStatus(),
        status: 'waiting',
        armed_at: new Date().toISOString(),
        active_dispatches: active,
        pid: null
      };
      writeState(armed);
      this.scheduleDrain();
      return { started: true, status: view(armed) };
    }
    this.launch();
    return { started: true, status: this.status() };
  }

  /** Cancels an armed (waiting) update. Anything further along runs to
   * completion -- a downloaded update must not be half-cancelled. */
  cancel(): boolean {
    const existing = this.read();
    if (existing.status !== 'waiting') return false;
    this.stopDrain();
    writeState({ ...idleStatus(), pid: null });
    return true;
  }

  private async countClaims(): Promise<number> {
    try {
      return await (this.deps.countActiveClaims ?? countActiveClaims)();
    } catch {
      // Claims cannot be read: arm anyway. The drain poll re-checks before
      // launching, and a failed check there re-arms rather than launches.
      return 0;
    }
  }

  private scheduleDrain(): void {
    if (this.drainTimer) return;
    this.drainTimer = setInterval(() => {
      void this.drainTick().catch(() => undefined);
    }, DRAIN_POLL_MS);
    this.drainTimer.unref?.();
  }

  private stopDrain(): void {
    if (this.drainTimer) {
      clearInterval(this.drainTimer);
      this.drainTimer = null;
    }
  }

  private async drainTick(): Promise<void> {
    const current = this.read();
    if (current.status !== 'waiting') {
      this.stopDrain();
      return;
    }
    const active = await this.countClaims();
    if (active > 0) {
      writeState({ ...current, active_dispatches: active });
      return;
    }
    this.stopDrain();
    this.launch();
  }

  private launch(): void {
    const child: ChildProcess = this.spawnFn(
      findGahBinary(),
      ['update', '--from-release', '--role', 'worker', '--repo', this.cwd],
      {
        cwd: this.cwd,
        env: process.env,
        detached: true,
        stdio: ['ignore', 'pipe', 'pipe']
      }
    );
    child.unref();

    let record: WorkerUpdateRecord = {
      ...idleStatus(),
      status: 'running',
      started_at: new Date().toISOString(),
      pid: child.pid ?? null
    };
    writeState(record);

    const appendOutput = (chunk: Buffer | string) => {
      record = {
        ...record,
        output: (record.output + chunk.toString()).slice(-MAX_OUTPUT_CHARS)
      };
      writeState(record);
    };
    child.stdout?.on('data', appendOutput);
    child.stderr?.on('data', appendOutput);

    child.on('close', (code) => {
      record = {
        ...record,
        status: code === 0 ? 'success' : 'failed',
        finished_at: new Date().toISOString(),
        active_dispatches: null
      };
      writeState(record);
      if (code === 0 && process.platform === 'darwin') {
        this.restartWorker();
      }
    });

    child.on('error', (error) => {
      record = {
        ...record,
        status: 'failed',
        finished_at: new Date().toISOString(),
        output: `${record.output}\n[gah] failed to spawn the updater: ${error.message}`.slice(-MAX_OUTPUT_CHARS)
      };
      writeState(record);
    });
  }

  /** Restart this worker's LaunchAgent so the node server runs the release
   * it just installed (the success state is already on disk for the freshly
   * started process -- and central -- to read). No-op where the worker is
   * not launchd-managed: the artifacts are installed and the next manual
   * restart picks them up. */
  private restartWorker(): void {
    if (this.deps.restartWorker) {
      this.deps.restartWorker();
      return;
    }
    const script = resolve(this.cwd, 'scripts/macos-launchd.sh');
    if (!existsSync(script)) return;
    try {
      const restart = spawn('bash', [script, 'start', 'worker'], {
        cwd: this.cwd,
        detached: true,
        stdio: 'ignore'
      });
      restart.unref();
    } catch {
      // The restart is best-effort; the install already succeeded.
    }
  }
}
