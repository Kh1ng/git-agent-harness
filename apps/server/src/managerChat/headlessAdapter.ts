/**
 * Headless (Tier B) manager-chat adapter (slice 4).
 *
 * For backends with no ACP or equivalent structured protocol (vibe, agy):
 * each turn is ONE non-interactive CLI invocation (`--print`-
 * style). Conversation context is reconstructed textually per turn — the
 * same replay approach the ACP path uses when a backend forgets its
 * history (see resumePrompt in acpAdapter.ts): full transcript replay for
 * a fresh process, since a one-shot process has no memory at all.
 *
 * The replayed prompt (full history + new message) is delivered to the
 * child process over stdin, never argv (issue #1009): a long-running
 * session's replayed transcript easily exceeds the OS ARG_MAX when passed
 * as a command-line argument, and spawn(2) fails with E2BIG before the
 * backend even starts. Argv stays a small, fixed set of flags regardless
 * of conversation length.
 *
 * What this buys: every backend in the unified chat surface, session
 * worktree binding (cwd per conversation), backend interchange, quota
 * handoff, and the event-sourced log — everything except streaming,
 * native slash commands, per-backend config options, and the permission
 * round-trip, which need a structured protocol.
 */

import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, realpathSync } from 'node:fs';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';
import type { ManagerAdapter, ManagerCommandInfo, ManagerModelInfo } from './registry.js';

export type { ManagerCommandInfo, ManagerModelInfo };

export interface HeadlessSpawnSpec {
  command: string;
  args: string[];
  /** Env for the child process. */
  env?: Record<string, string>;
}

interface HeadlessProcessResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
}

/** Builds the CLI argv for one turn: cwd is passed by the engine; the spec
 * adds backend-specific flags. Bounded and content-free — never derived
 * from the prompt or history, so argv size cannot grow with conversation
 * length (issue #1009). */
export interface HeadlessBackendSpec {
  id: string;
  displayName: string;
  /** Fixed argv for a print-mode turn. Must not embed prompt/history. */
  turnArgs: () => string[];
  /** Encode this turn's full prompt (history already replayed in) for the
   * backend's stdin channel. */
  encodeStdin: (prompt: string) => string;
  /** Extract the reply text from a finished process, or throw a
   * descriptive error. Default: trimmed stdout on exit 0, else an error
   * built from stderr. */
  parseReply?: (result: HeadlessProcessResult) => string;
}

interface ConversationState {
  knownHistory: ChatTranscriptTurn[];
  /** Process handle for the in-flight turn (cancel support). */
  child: ReturnType<typeof spawn> | null;
}

/** Mirrors acpAdapter's resumePrompt: fresh process => replay the whole
 * conversation as text before the new prompt. */
function replayPrompt(message: string, history: ChatTranscriptTurn[]): string {
  if (history.length === 0) return message;
  const lines = history.map((turn) => `${turn.role}: ${turn.text}`);
  return `${lines.join('\n')}\n\nuser: ${message}`;
}

function defaultParseReply(displayName: string): (result: HeadlessProcessResult) => string {
  return ({ stdout, stderr, exitCode }) => {
    if (exitCode !== 0) {
      const detail = stderr.trim().slice(0, 400) || `exit code ${exitCode}`;
      throw new Error(`${displayName} turn failed: ${detail}`);
    }
    return stdout.trim();
  };
}

const TURN_TIMEOUT_MS = 10 * 60_000;

export function createHeadlessBackend(spec: HeadlessBackendSpec): ManagerAdapter {
  const states = new Map<string, ConversationState>();
  const parseReply = spec.parseReply ?? defaultParseReply(spec.displayName);

  function stateFor(key: string): ConversationState {
    let state = states.get(key);
    if (!state) {
      state = { knownHistory: [], child: null };
      states.set(key, state);
    }
    return state;
  }

  return {
    id: spec.id,
    displayName: spec.displayName,
    implemented: true,

    async runTurn(gahProfile, input) {
      const state = stateFor(gahProfile);
      // A headless process has no memory: every turn replays the full
      // conversation. (historyDelta-style catch-up is meaningless here, but
      // keeping knownHistory lets future stream-json modes upgrade in place.)
      const prompt = replayPrompt(input.prompt, input.history);
      const cwd = input.cwd ?? process.cwd();

      const args = spec.turnArgs();
      const child = spawn(args[0], args.slice(1), {
        cwd,
        env: { ...process.env },
        stdio: ['pipe', 'pipe', 'pipe']
      });
      state.child = child;

      let stdout = '';
      let stderr = '';
      child.stdout.on('data', (chunk) => { stdout += chunk; });
      child.stderr.on('data', (chunk) => { stderr += chunk; });
      // The child may exit (e.g. a fast validation failure) before we
      // finish writing; a write past that point would otherwise raise an
      // unhandled EPIPE and crash the server.
      child.stdin.on('error', () => {});
      child.stdin.write(spec.encodeStdin(prompt));
      child.stdin.end();

      const killTimer = setTimeout(() => {
        child.kill('SIGKILL');
      }, TURN_TIMEOUT_MS);
      killTimer.unref?.();

      try {
        const code = await new Promise<number | null>((resolve, reject) => {
          child.on('error', reject);
          child.on('close', (exitCode) => resolve(exitCode));
        });
        const reply = parseReply({ stdout, stderr, exitCode: code });
        if (reply.trim().length === 0) {
          throw new Error(`${spec.displayName} turn produced no output.`);
        }
        state.knownHistory = [
          ...input.history,
          { role: 'user', text: input.prompt, timestamp: Date.now() },
          { role: 'assistant', text: reply, timestamp: Date.now() }
        ];
        return { reply, model: null, usage: null };
      } finally {
        clearTimeout(killTimer);
        state.child = null;
      }
    },

    async listCommands(): Promise<ManagerCommandInfo[]> {
      return []; // no native slash commands over a one-shot pipe
    },

    async listModels() {
      return {
        models: [],
        currentModelId: null,
        reasoningEfforts: [],
        currentReasoningEffortId: null
      }; // no config-option protocol
    },

    async setModel(): Promise<void> {
      throw new Error(`${spec.displayName} doesn't support model selection in headless mode.`);
    },

    async setReasoningEffort(): Promise<void> {
      throw new Error(`${spec.displayName} doesn't support reasoning-effort selection in headless mode.`);
    },

    async steerTurn(): Promise<{ outcome: 'injected' }> {
      throw new Error(`${spec.displayName} doesn't support mid-turn steering in headless mode.`);
    },

    async cancelTurn(gahProfile): Promise<void> {
      const state = states.get(gahProfile);
      if (state?.child) {
        state.child.kill('SIGTERM');
        setTimeout(() => state.child?.kill('SIGKILL'), 3000).unref?.();
      }
    }
  };
}

/** Fixed (content-free) bootstrap handed to vibe's own Python interpreter
 * via `-c`. It drains the prompt from its stdin, sets the *in-process*
 * `sys.argv` — never the OS-level exec argv — and calls the same
 * `vibe.cli.entrypoint.main` the real `vibe` launcher script calls. Because
 * sys.argv is assigned after the interpreter is already running, this never
 * goes through execve() with the prompt as an argument, so it stays outside
 * ARG_MAX regardless of size (issue #1009).
 *
 * This also sidesteps a real bug in the installed vibe CLI: its
 * `get_prompt_from_stdin()` (vibe/cli/cli.py) reads a piped prompt
 * correctly, then unconditionally tries to reopen `/dev/tty` to restore
 * interactive stdin — which raises OSError with no controlling terminal
 * (as under this server), and the handler discards the prompt it just read,
 * so `vibe -p` with a piped prompt silently fails outside a TTY. Because
 * this bridge fully drains stdin itself before vibe's own code ever runs,
 * vibe's later get_prompt_from_stdin() call sees EOF and returns None
 * immediately — args.prompt (set directly on sys.argv) wins, and the
 * /dev/tty path is never reached. Confirmed against the installed CLI. */
const VIBE_STDIN_BRIDGE = [
  'import sys',
  'prompt = sys.stdin.buffer.read().decode("utf-8", "replace")',
  'sys.argv = ["vibe", "-p", prompt, "--output", "text"]',
  'from vibe.cli.entrypoint import main',
  'sys.exit(main())'
].join('\n');

/** Resolve the Python interpreter backing the installed `vibe` launcher
 * from its own shebang, rather than hard-coding a host-specific path. Not
 * cached: it's one cheap `command -v` + file read per turn, and caching
 * would make the resolved interpreter outlive a test's fake PATH. */
function resolveVibeInterpreter(): string {
  const launcherPath = execFileSync('/bin/sh', ['-c', 'command -v vibe'], { encoding: 'utf8' }).trim();
  if (!launcherPath) {
    throw new Error('vibe executable not found on PATH.');
  }
  const realLauncher = realpathSync(launcherPath);
  const shebang = readFileSync(realLauncher, 'utf8').split('\n', 1)[0];
  const match = /^#!(\S+)/.exec(shebang);
  if (!match) {
    throw new Error(`Could not determine vibe's Python interpreter from ${realLauncher}.`);
  }
  return match[1];
}

/** vibe: Mistral's CLI, print mode, driven through the stdin bridge above
 * so the replayed transcript never reaches process argv.
 * `resolveInterpreter` is overridable so tests can prove the argv/stdin
 * split against a fake interpreter instead of requiring a real vibe
 * install. */
export function vibeBackendSpec(overrides: { resolveInterpreter?: () => string } = {}): HeadlessBackendSpec {
  const resolveInterpreter = overrides.resolveInterpreter ?? resolveVibeInterpreter;
  return {
    id: 'vibe',
    displayName: 'Vibe',
    turnArgs: () => [resolveInterpreter(), '-c', VIBE_STDIN_BRIDGE],
    encodeStdin: (prompt) => prompt
  };
}

interface AgyStreamResult {
  status: 'SUCCESS' | 'ERROR' | string;
  response?: string;
  error?: string;
}

/** agy: print mode with NDJSON stdin/stdout so the prompt never touches
 * argv (issue #1009). `--input-format stream-json` reads one `{"event":
 * "user", "message": {"content": [{"type": "text", "text": "..."}]}}`
 * line per turn; `--output-format stream-json` emits one JSON object per
 * line, terminated by an `{"event": "result", "result": {...}}` line
 * carrying the final text and status. This exact shape was confirmed
 * against the installed CLI (its own errors named the required "event",
 * "message", and "content" fields one at a time) — none of it is
 * documented in `agy --help`.
 *
 * Headless has no permission round-trip (stdin is the prompt channel, not
 * a TTY), so without --dangerously-skip-permissions agy auto-denies every
 * tool call and the turn silently degrades to a no-tool reply. --sandbox
 * is kept alongside it so the auto-approved tools still run confined to
 * the session cwd. */
export function agyBackendSpec(): HeadlessBackendSpec {
  return {
    id: 'agy',
    displayName: 'Agy',
    turnArgs: () => [
      'agy',
      '--input-format',
      'stream-json',
      '--output-format',
      'stream-json',
      '--dangerously-skip-permissions',
      '--sandbox'
    ],
    encodeStdin: (prompt) =>
      `${JSON.stringify({
        event: 'user',
        message: { content: [{ type: 'text', text: prompt }] }
      })}\n`,
    parseReply: ({ stdout, stderr, exitCode }) => {
      const lines = stdout.split('\n');
      for (let i = lines.length - 1; i >= 0; i--) {
        const line = lines[i].trim();
        if (!line) continue;
        let event: { event?: string; result?: AgyStreamResult };
        try {
          event = JSON.parse(line);
        } catch {
          continue;
        }
        if (event.event !== 'result' || !event.result) continue;
        const result = event.result;
        if (result.status === 'SUCCESS') return result.response ?? '';
        const detail = result.error || stderr.trim().slice(0, 400) || `exit code ${exitCode}`;
        throw new Error(`Agy turn failed: ${detail}`);
      }
      // No result line at all: a fatal failure before agy could emit one
      // (missing binary, killed by signal, crash).
      const detail = stderr.trim().slice(0, 400) || `exit code ${exitCode}`;
      throw new Error(`Agy turn failed: ${detail}`);
    }
  };
}
