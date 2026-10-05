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

/** What is known about a conversation without reading any prompt. */
export interface SessionDescription { title: string | null; last_activity_at: string | null; model?: string | null }
export type SessionDescriber = (tool: string, info: ProcessInfo) => SessionDescription | null;

const TAIL_BYTES = 512 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function readSlice(file: string, start: number, length: number): string {
  const buffer = Buffer.alloc(length);
  const fd = openSync(file, 'r');
  try { return buffer.toString('utf8', 0, readSync(fd, buffer, 0, length, start)); } finally { closeSync(fd); }
}

/** Claude Code's transcript directory for a working directory. */
function claudeProjectDir(home: string, cwd: string): string {
  return join(home, '.claude', 'projects', cwd.replace(/[^a-zA-Z0-9]/g, '-'));
}

/** The transcript of a known session, wherever its project directory is. */
function claudeTranscript(sessionId: string, home: string): string | null {
  const projects = join(home, '.claude', 'projects');
  try {
    for (const project of readdirSync(projects)) {
      const candidate = join(projects, project, `${sessionId}.jsonl`);
      try { statSync(candidate); return candidate; } catch { /* Not in this project. */ }
    }
  } catch { /* No Claude Code state on this host. */ }
  return null;
}

/** A process started without a session id wrote its transcript's first
 * record moments after it started: the closest such transcript in its
 * working directory's project, within three minutes, is its own. */
function claudeTranscriptByStart(info: ProcessInfo, home: string): string | null {
  if (!info.cwd || !info.startedAt) return null;
  const directory = claudeProjectDir(home, info.cwd);
  const started = Date.parse(info.startedAt);
  let best: { file: string; gap: number } | null = null;
  try {
    for (const name of readdirSync(directory)) {
      if (!name.endsWith('.jsonl') || !UUID.test(name.slice(0, -'.jsonl'.length))) continue;
      const file = join(directory, name);
      const first = /"timestamp":"([^"]+)"/.exec(readSlice(file, 0, 4096));
      const gap = first ? Date.parse(first[1]) - started : NaN;
      if (Number.isFinite(gap) && gap >= -5_000 && gap < 180_000 && (!best || Math.abs(gap) < Math.abs(best.gap))) best = { file, gap };
    }
  } catch { /* No transcripts for this directory. */ }
  return best?.file ?? null;
}

/** Titles and models the Claude desktop app keeps per CLI session. */
export function claudeDesktopSessions(home = homedir()): Map<string, { title: string | null; model: string | null }> {
  const sessions = new Map<string, { title: string | null; model: string | null }>();
  for (const root of [join(home, '.config', 'Claude'), join(home, 'Library', 'Application Support', 'Claude')]) {
    const base = join(root, 'claude-code-sessions');
    try {
      for (const account of readdirSync(base)) for (const workspace of readdirSync(join(base, account))) {
        for (const name of readdirSync(join(base, account, workspace))) {
          if (!name.endsWith('.json')) continue;
          try {
            const record = JSON.parse(readFileSync(join(base, account, workspace, name), 'utf8')) as { cliSessionId?: unknown; title?: unknown; model?: unknown };
            if (typeof record.cliSessionId !== 'string') continue;
            sessions.set(record.cliSessionId, {
              title: typeof record.title === 'string' && record.title.trim() ? record.title.trim().slice(0, 120) : null,
              model: typeof record.model === 'string' && record.model ? record.model : null
            });
          } catch { /* A record being rewritten. */ }
        }
      }
    } catch { /* The desktop app is not installed here. */ }
  }
  return sessions;
}

/**
 * A Claude Code conversation's title, model and last write. The title is the
 * desktop app's, else the transcript's last `ai-title` / `custom-title`
 * record; the model is the desktop app's, else the last assistant turn's.
 * Nothing a person typed is read out.
 */
export function describeClaudeTranscript(file: string, desktop?: { title: string | null; model: string | null }): SessionDescription {
  const stat = statSync(file);
  let title: string | null = null;
  let model: string | null = null;
  try {
    const length = Math.min(stat.size, TAIL_BYTES);
    for (const line of readSlice(file, stat.size - length, length).split('\n')) {
      if (line.includes('"ai-title"') || line.includes('"custom-title"')) {
        try {
          const record = JSON.parse(line) as { type?: string; aiTitle?: unknown; customTitle?: unknown };
          const value = record.type === 'ai-title' ? record.aiTitle : record.type === 'custom-title' ? record.customTitle : null;
          if (typeof value === 'string' && value.trim()) title = value.trim().slice(0, 120);
        } catch { /* A line cut by the tail window. */ }
      } else if (line.includes('"type":"assistant"')) {
        const match = /"model":"([\w.:\-\[\]/]{1,80})"/.exec(line);
        if (match && !match[1].startsWith('<')) model = match[1];
      }
    }
  } catch { /* Title and model stay unknown. */ }
  return { title: desktop?.title ?? title, model: desktop?.model ?? model, last_activity_at: stat.mtime.toISOString() };
}

export function describeClaudeSession(sessionId: string, home = homedir()): SessionDescription | null {
  const file = claudeTranscript(sessionId, home);
  return file ? describeClaudeTranscript(file, claudeDesktopSessions(home).get(sessionId)) : null;
}

/** The describer for one scan: desktop records are read once. */
export function claudeDescriber(home = homedir()): SessionDescriber {
  let desktop: ReturnType<typeof claudeDesktopSessions> | null = null;
  return (tool, info) => {
    if (tool !== 'claude') return null;
    const sessionId = agentSessionId(info.argv);
    const file = sessionId ? claudeTranscript(sessionId, home) : claudeTranscriptByStart(info, home);
    if (!file) return null;
    desktop ??= claudeDesktopSessions(home);
    return describeClaudeTranscript(file, desktop.get(basename(file, '.jsonl')));
  };
}

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
    const described = describe(tool, info);
    // `--model default` names no model; the conversation's own record does.
    const model = row.model && row.model !== 'default' ? row.model : described?.model ?? null;
    agents.push(described ? { ...row, title: described.title, last_activity_at: described.last_activity_at, model } : { ...row, model });
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
  return { supported: true, generated_at, ...classifyAgents(readLinuxProcesses(), factoryRoots, process.pid, claudeDescriber()) };
}
