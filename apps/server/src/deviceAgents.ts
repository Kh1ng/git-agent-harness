import { closeSync, openSync, readdirSync, readFileSync, readlinkSync, readSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join, sep } from 'node:path';
import type { DeviceAgent, DeviceAgentsSnapshot } from '@git-agent-harness/contracts';

/** Coding-agent CLIs worth listing, by executable or script name. */
const TOOLS = ['claude', 'codex', 'gemini', 'opencode', 'vibe', 'aider', 'agy'] as const;

/** What one process looks like to the scanner; `/proc` supplies it in production. */
export interface ProcessInfo {
  pid: number;
  ppid: number | null;
  /** argv, as the process was started. */
  argv: string[];
  cwd: string | null;
  startedAt: string | null;
}

/** The agent CLI a command line runs, if any: the executable itself, or the
 * script a node/bun/python launcher was handed (`node /usr/bin/claude`). */
export function agentTool(argv: string[]): string | null {
  const names = argv.slice(0, 2).map((part) => basename(part).toLowerCase().replace(/\.(js|mjs|cjs|py|exe)$/, ''));
  const [first, second] = names;
  const match = (name: string | undefined) => TOOLS.find((tool) => name === tool || name === `${tool}-code` || name === `${tool}-cli`) ?? null;
  if (!first) return null;
  if (/^(node|bun|deno|python\d*(\.\d+)?|npx|tsx)$/.test(first)) return match(second);
  return match(first);
}

/** The value of a `--model` / `-m` flag, and nothing else from the command line. */
export function agentModel(argv: string[]): string | null {
  for (let index = 1; index < argv.length; index++) {
    const part = argv[index];
    const value = part === '--model' || part === '-m' ? argv[index + 1] : part.startsWith('--model=') ? part.slice('--model='.length) : null;
    if (value && /^[\w.:\-\[\]/]{1,80}$/.test(value)) return value;
    if (value !== null) return null;
  }
  return null;
}

/** The conversation a Claude Code process serves (`--resume=<id>` or `--session-id=<id>`). */
export function agentSessionId(argv: string[]): string | null {
  for (let index = 1; index < argv.length; index++) {
    const match = /^--(?:resume|session-id)(?:=(.+))?$/.exec(argv[index]);
    if (!match) continue;
    const value = match[1] ?? argv[index + 1] ?? '';
    return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) ? value : null;
  }
  return null;
}

/** What a transcript says about its conversation without reading any prompt. */
export type SessionDescriber = (tool: string, argv: string[]) => { title: string | null; last_activity_at: string | null } | null;

const TITLE_TAIL_BYTES = 512 * 1024;

/**
 * A Claude Code conversation's title and last write, from its transcript
 * under `<home>/.claude/projects/<project>/<session>.jsonl`. Only the
 * `ai-title` / `custom-title` records are read out; the last one wins.
 */
export function describeClaudeSession(sessionId: string, home = homedir()): { title: string | null; last_activity_at: string | null } | null {
  const projects = join(home, '.claude', 'projects');
  let file: string | null = null;
  try {
    for (const project of readdirSync(projects)) {
      const candidate = join(projects, project, `${sessionId}.jsonl`);
      try { statSync(candidate); file = candidate; break; } catch { /* Not in this project. */ }
    }
  } catch {
    return null;
  }
  if (!file) return null;
  const stat = statSync(file);
  let title: string | null = null;
  try {
    const length = Math.min(stat.size, TITLE_TAIL_BYTES);
    const buffer = Buffer.alloc(length);
    const fd = openSync(file, 'r');
    try { readSync(fd, buffer, 0, length, stat.size - length); } finally { closeSync(fd); }
    for (const line of buffer.toString('utf8').split('\n')) {
      if (!line.includes('"ai-title"') && !line.includes('"custom-title"')) continue;
      try {
        const record = JSON.parse(line) as { type?: string; aiTitle?: unknown; customTitle?: unknown };
        const value = record.type === 'ai-title' ? record.aiTitle : record.type === 'custom-title' ? record.customTitle : null;
        if (typeof value === 'string' && value.trim()) title = value.trim().slice(0, 120);
      } catch { /* A line cut by the tail window. */ }
    }
  } catch { /* The title stays unknown. */ }
  return { title, last_activity_at: stat.mtime.toISOString() };
}

const describeSession: SessionDescriber = (tool, argv) => {
  if (tool !== 'claude') return null;
  const sessionId = agentSessionId(argv);
  return sessionId ? describeClaudeSession(sessionId) : null;
};

function inside(path: string, root: string): boolean {
  const base = root.endsWith(sep) ? root : root + sep;
  return path === root || path.startsWith(base);
}

/**
 * Every agent CLI on this device, split by where it runs: inside a factory
 * root (a profile's worktree base) the factory started it; anywhere else it
 * is someone's own session. An agent's helper processes (same tool, started
 * by it) are folded into it. Rows are newest first.
 */
export function classifyAgents(processes: ProcessInfo[], factoryRoots: string[], selfPid = process.pid, describe: SessionDescriber = () => null): { agents: DeviceAgent[]; factory_agents: DeviceAgent[] } {
  const roots = factoryRoots.filter((root) => root && root !== sep);
  const tools = new Map<number, string>();
  for (const info of processes) {
    const tool = info.pid === selfPid ? null : agentTool(info.argv);
    if (tool) tools.set(info.pid, tool);
  }
  const agents: DeviceAgent[] = [];
  const factory_agents: DeviceAgent[] = [];
  for (const info of processes) {
    const tool = tools.get(info.pid);
    if (!tool) continue;
    if (info.ppid !== null && tools.get(info.ppid) === tool) continue;
    const row: DeviceAgent = { pid: info.pid, tool, cwd: info.cwd, started_at: info.startedAt, model: agentModel(info.argv) };
    if (info.cwd && roots.some((root) => inside(info.cwd!, root))) { factory_agents.push(row); continue; }
    // Only someone's own session gets its title looked up; factory jobs are named by their work id.
    const described = describe(tool, info.argv);
    agents.push(described ? { ...row, ...described } : row);
  }
  const newestFirst = (a: DeviceAgent, b: DeviceAgent) => (b.started_at ?? '').localeCompare(a.started_at ?? '') || a.pid - b.pid;
  return { agents: agents.sort(newestFirst), factory_agents: factory_agents.sort(newestFirst) };
}

/** Boot time in epoch ms, from /proc/stat's btime. */
function bootTimeMs(): number | null {
  try {
    const match = /^btime (\d+)$/m.exec(readFileSync('/proc/stat', 'utf8'));
    return match ? Number(match[1]) * 1000 : null;
  } catch {
    return null;
  }
}

/** Every readable process under /proc. Processes of other users simply lack a cwd. */
export function readLinuxProcesses(): ProcessInfo[] {
  const boot = bootTimeMs();
  const processes: ProcessInfo[] = [];
  for (const entry of readdirSync('/proc')) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const argv = readFileSync(`/proc/${entry}/cmdline`, 'utf8').split('\0').filter(Boolean);
      if (argv.length === 0) continue;
      let cwd: string | null = null;
      try { cwd = readlinkSync(`/proc/${entry}/cwd`); } catch { /* Not ours to read. */ }
      let startedAt: string | null = null;
      if (boot !== null) {
        // Field 22 (starttime, clock ticks since boot) follows the parenthesised command name.
        const stat = readFileSync(`/proc/${entry}/stat`, 'utf8');
        const ticks = Number(stat.slice(stat.lastIndexOf(')') + 2).split(' ')[19]);
        if (Number.isFinite(ticks)) startedAt = new Date(boot + (ticks / 100) * 1000).toISOString();
      }
      // Field 4 (ppid) is the second one after the command name.
      const status = readFileSync(`/proc/${entry}/stat`, 'utf8');
      const ppid = Number(status.slice(status.lastIndexOf(')') + 2).split(' ')[1]);
      processes.push({ pid: Number(entry), ppid: Number.isFinite(ppid) ? ppid : null, argv, cwd, startedAt });
    } catch {
      // The process exited between the listing and the read.
    }
  }
  return processes;
}

export function deviceAgentsSnapshot(factoryRoots: string[]): DeviceAgentsSnapshot {
  const generated_at = new Date().toISOString();
  if (process.platform !== 'linux') return { supported: false, generated_at, agents: [], factory_agents: [] };
  return { supported: true, generated_at, ...classifyAgents(readLinuxProcesses(), factoryRoots, process.pid, describeSession) };
}
