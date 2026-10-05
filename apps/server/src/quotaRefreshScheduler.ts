import { spawn, type ChildProcess } from 'node:child_process';
import { findGahBinary, getSpawnOptions, terminateProcessTree } from './gahCli.js';

/** Every node observes its own installed accounts, independently of factory
 * work or an open dashboard. The native command applies its own freshness
 * throttle and walks the configured profiles; credentials stay on this node. */
export function startQuotaRefreshScheduler(deps: {
  spawn?: () => ChildProcess;
  terminate?: (child: ChildProcess) => Promise<unknown>;
  intervalMs?: number;
  timeoutMs?: number;
} = {}): () => Promise<void> {
  const spawnRefresh = deps.spawn ?? (() => spawn(findGahBinary(), ['quota', 'auto-refresh'], {
    ...getSpawnOptions(undefined, true), stdio: 'ignore'
  }));
  const terminate = deps.terminate ?? (async (child: ChildProcess) => {
    if (child.pid) await terminateProcessTree(child.pid, child);
  });
  let stopped = false;
  let child: ChildProcess | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  let stopping: Promise<unknown> | undefined;

  const kill = () => {
    if (stopping) return stopping;
    if (!child) return Promise.resolve();
    return stopping = terminate(child).catch(() => undefined).finally(() => { stopping = undefined; });
  };
  const tick = () => {
    if (stopped || child || stopping) return;
    let next: ChildProcess;
    try { next = spawnRefresh(); } catch { return; }
    child = next;
    const finish = () => {
      if (child !== next) return;
      clearTimeout(timeout);
      child = undefined;
    };
    next.once('error', finish);
    next.once('close', finish);
    timeout = setTimeout(() => { void kill(); }, deps.timeoutMs ?? 300_000);
    timeout.unref?.();
  };
  // Matches the systemd timer; the native 14-minute throttle makes every tick useful (#1331).
  const timer = setInterval(tick, deps.intervalMs ?? 15 * 60_000);
  timer.unref?.();
  tick();
  return async () => {
    stopped = true;
    clearInterval(timer);
    clearTimeout(timeout);
    await kill();
  };
}
