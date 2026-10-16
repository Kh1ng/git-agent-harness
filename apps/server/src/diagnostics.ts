import { execFile } from 'node:child_process';
import { appendFile, mkdir, readFile, rename, stat } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { homedir, hostname, arch, cpus, freemem, loadavg, platform, release, totalmem, uptime } from 'node:os';
import { promisify } from 'node:util';
import { gzipSync } from 'node:zlib';
import { COORDINATOR_VERSION } from '@git-agent-harness/contracts';

const execFileAsync = promisify(execFile);

const SAMPLE_INTERVAL_MS = 10_000;
const TOP_PROCESSES = 20;
// ponytail: two 5 MB files hold about 15 hours of 10-second samples; raise the
// cap if testers need a longer window.
const MAX_TIMELINE_BYTES = 5 * 1024 * 1024;

export interface ProcessSample { pid: number; ppid: number; rss_kb: number; cpu: number; cmd: string }
export interface MemorySample {
  t: string;
  total_bytes: number;
  free_bytes: number;
  load1: number;
  server: { pid: number; rss_bytes: number; heap_used_bytes: number; external_bytes: number };
  processes: ProcessSample[];
}

/** Outside the Git checkout, same reasoning as adminUpdate.ts. */
export function diagnosticsDir(): string {
  return process.env.GAH_DIAGNOSTICS_DIR ||
    resolve(process.env.XDG_STATE_HOME || resolve(homedir(), '.local', 'state'), 'gah', 'diagnostics');
}

/** Parses `ps -axo pid=,ppid=,rss=,pcpu=,comm=`; rss is KiB on macOS and Linux.
 * Keeps the largest processes plus every descendant of the server, since agent
 * CLIs the server starts matter even while they are small. Command names only:
 * arguments can carry tokens. */
export function parseProcesses(text: string, serverPid: number, limit = TOP_PROCESSES): ProcessSample[] {
  const all: ProcessSample[] = [];
  for (const line of text.split('\n')) {
    const match = /^\s*(\d+)\s+(\d+)\s+(\d+)\s+([\d.]+)\s+(.+?)\s*$/.exec(line);
    if (!match) continue;
    all.push({ pid: +match[1], ppid: +match[2], rss_kb: +match[3], cpu: +match[4], cmd: basename(match[5]) });
  }
  const children = new Map<number, number[]>();
  for (const p of all) children.set(p.ppid, [...(children.get(p.ppid) ?? []), p.pid]);
  const ours = new Set<number>([serverPid]);
  const queue = [serverPid];
  while (queue.length) for (const child of children.get(queue.pop()!) ?? []) if (!ours.has(child)) { ours.add(child); queue.push(child); }
  const sorted = all.sort((a, b) => b.rss_kb - a.rss_kb);
  const top = new Set(sorted.slice(0, limit).map((p) => p.pid));
  return sorted.filter((p) => top.has(p.pid) || ours.has(p.pid));
}

export async function takeSample(): Promise<MemorySample> {
  let processes: ProcessSample[] = [];
  try {
    const { stdout } = await execFileAsync('ps', ['-axo', 'pid=,ppid=,rss=,pcpu=,comm='], { maxBuffer: 8 * 1024 * 1024, timeout: 5_000 });
    processes = parseProcesses(stdout, process.pid);
  } catch {
    // No ps (or it failed): host totals and the server's own numbers still help.
  }
  const memory = process.memoryUsage();
  return {
    t: new Date().toISOString(),
    total_bytes: totalmem(),
    free_bytes: freemem(),
    load1: loadavg()[0] ?? 0,
    server: { pid: process.pid, rss_bytes: memory.rss, heap_used_bytes: memory.heapUsed, external_bytes: memory.external },
    processes
  };
}

async function appendSample(dir: string): Promise<void> {
  const sample = await takeSample();
  await mkdir(dir, { recursive: true });
  const current = resolve(dir, 'memory.jsonl');
  const size = await stat(current).then((s) => s.size, () => 0);
  if (size > MAX_TIMELINE_BYTES) await rename(current, resolve(dir, 'memory.1.jsonl'));
  await appendFile(current, `${JSON.stringify(sample)}\n`);
}

/** Records a memory sample every 10 seconds for the diagnostics bundle. */
export function startMemorySampler(dir = diagnosticsDir()): () => void {
  const tick = () => { appendSample(dir).catch((error) => console.error('[diagnostics] memory sample failed:', error)); };
  tick();
  const timer = setInterval(tick, SAMPLE_INTERVAL_MS);
  timer.unref();
  return () => clearInterval(timer);
}

async function readTimeline(dir: string): Promise<MemorySample[]> {
  const samples: MemorySample[] = [];
  for (const name of ['memory.1.jsonl', 'memory.jsonl']) {
    const text = await readFile(resolve(dir, name), 'utf8').catch(() => '');
    for (const line of text.split('\n')) {
      if (!line) continue;
      try { samples.push(JSON.parse(line) as MemorySample); } catch { /* a torn last line from a crash */ }
    }
  }
  return samples;
}

/** Gzipped JSON a tester sends back when something misbehaves. */
export async function buildDiagnosticsBundle(nodeRole: string, dir = diagnosticsDir()): Promise<Buffer> {
  const bundle = {
    format: 'parasaur-diagnostics/1',
    generated_at: new Date().toISOString(),
    version: COORDINATOR_VERSION,
    node_role: nodeRole,
    host: { hostname: hostname(), platform: platform(), release: release(), arch: arch(), cpus: cpus().length, uptime_s: Math.round(uptime()) },
    server_uptime_s: Math.round(process.uptime()),
    current: await takeSample(),
    timeline: await readTimeline(dir)
  };
  return gzipSync(JSON.stringify(bundle));
}
