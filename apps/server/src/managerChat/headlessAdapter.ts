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
import path from 'node:path';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';
import type { ManagerAdapter, ManagerCommandInfo, ManagerModelInfo } from './registry.js';

export type { ManagerCommandInfo, ManagerModelInfo };

export interface HeadlessSpawnSpec {
  command: string;
  args: string[];
  /** Env for the child process. */
  env?: Record<string, string>;
}

/** A backend tool request decoded out of reply text (#1041). */
export interface HeadlessToolRequest {
  name: string;
  args: Record<string, unknown>;
  /** The exact reply the request was decoded from, replayed back to the
   * backend so its own context stays intact across the continuation. */
  raw: string;
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
  /** Argv for a print-mode turn. Must not embed prompt/history -- only the
   * session's pinned model/reasoning-effort (#1032 reopened), which are
   * bounded ids, not conversation content. A spec that has no native flag
   * for one or both (e.g. vibe) may ignore the argument entirely. */
  turnArgs: (opts?: { model?: string | null; reasoningEffort?: string | null }) => string[];
  /** Encode this turn's full prompt (history already replayed in) for the
   * backend's stdin channel. */
  encodeStdin: (prompt: string) => string;
  /** Extract the reply text from a finished process, or throw a
   * descriptive error. Default: trimmed stdout on exit 0, else an error
   * built from stderr. */
  parseReply?: (result: HeadlessProcessResult) => string;
  /** Detect a tool request the backend leaked into the reply text instead
   * of executing (#1041). Returns null for an ordinary reply; throws when
   * tool-request-shaped text cannot be decoded, so the turn ends with an
   * actionable error instead of a false-success reply. */
  decodeToolRequest?: (reply: string) => HeadlessToolRequest | null;
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

/** The continuation prompt after servicing a decoded tool request (#1041):
 * the same roleful replay format with the backend's own request and the
 * tool result appended, so the model continues from the result. */
function continuationPrompt(message: string, history: ChatTranscriptTurn[], exchange: ChatTranscriptTurn[]): string {
  const lines = history.map((turn) => `${turn.role}: ${turn.text}`);
  const prefix = lines.length > 0 ? `${lines.join('\n')}\n\n` : '';
  return `${prefix}user: ${message}\n${exchange.map((turn) => `${turn.role}: ${turn.text}`).join('\n')}`;
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
      const cwd = input.cwd ?? process.cwd();

      // One non-interactive invocation: fixed argv, prompt over stdin.
      const invoke = async (prompt: string): Promise<string> => {
        const args = spec.turnArgs({ model: input.model, reasoningEffort: input.reasoningEffort });
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
          return reply;
        } finally {
          clearTimeout(killTimer);
          state.child = null;
        }
      };

      // A headless process has no memory: every turn replays the full
      // conversation. (historyDelta-style catch-up is meaningless here, but
      // keeping knownHistory lets future stream-json modes upgrade in place.)
      let prompt = replayPrompt(input.prompt, input.history);
      let reply = await invoke(prompt);

      // #1041: a print-mode backend whose model emits a tool call the CLI
      // didn't execute leaks the raw request syntax as reply text, which
      // used to surface verbatim as a successful assistant reply. Decode
      // it, gate it through the same permission round-trip the ACP backends
      // use, execute the servable read-only subset GAH-side, then continue
      // the turn with the tool result replayed into the next invocation.
      // Undecodable or unknown requests fail the turn with an actionable
      // error instead.
      const toolExchange: ChatTranscriptTurn[] = [];
      if (spec.decodeToolRequest) {
        for (let round = 1; ; round += 1) {
          const request = spec.decodeToolRequest(reply);
          if (!request) break;
          if (round > MAX_TOOL_ROUNDS_PER_TURN) {
            throw new Error(`${spec.displayName} requested more than ${MAX_TOOL_ROUNDS_PER_TURN} tool calls in a single turn; stopping so raw tool syntax can never surface as a reply.`);
          }
          const plan = planHeadlessTool(spec.displayName, request.name, request.args, cwd);
          const toolCallId = `${spec.id}-tool-${Date.now().toString(36)}-${round}`;
          const emitToolCall = (status: 'pending' | 'completed' | 'failed', summary: string | null) =>
            input.onToolCall?.({
              toolCallId,
              name: request.name,
              title: plan.title,
              kind: plan.kind,
              status,
              locations: plan.locations,
              summary
            });
          emitToolCall('pending', null);
          let decision: string;
          if (input.requestPermission) {
            decision = await input.requestPermission({
              title: plan.title,
              options: [
                { optionId: 'allow-once', name: 'Allow', kind: 'allow_once' },
                { optionId: 'reject-once', name: 'Reject', kind: 'reject_once' }
              ],
              locations: plan.locations
            });
          } else {
            console.warn(`[managerChat] ${spec.displayName} requested permission for "${plan.title}" -- declining (no permission UI attached)`);
            decision = 'cancelled';
          }
          if (decision !== 'allow-once') {
            const outcome = decision === 'cancelled' ? 'cancelled' : `declined (${decision})`;
            const message = `${spec.displayName} requested "${plan.title}" but the permission request was ${outcome}; the turn cannot continue without it.`;
            emitToolCall('failed', message);
            throw new Error(message);
          }
          let output: string;
          try {
            output = await plan.run();
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            emitToolCall('failed', summarizeOutput(message));
            throw error;
          }
          emitToolCall('completed', summarizeOutput(output));
          input.onToolResult(request.name, output);
          toolExchange.push(
            { role: 'assistant', text: request.raw, timestamp: Date.now() },
            { role: 'tool', text: output, timestamp: Date.now() }
          );
          prompt = continuationPrompt(input.prompt, input.history, toolExchange);
          reply = await invoke(prompt);
        }
      }

      state.knownHistory = [
        ...input.history,
        { role: 'user', text: input.prompt, timestamp: Date.now() },
        ...toolExchange,
        { role: 'assistant', text: reply, timestamp: Date.now() }
      ];
      return { reply, model: null, usage: null };
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

/** Fixed `-c` bootstrap for vibe's own Python interpreter: reads the prompt
 * from stdin, sets it into *in-process* `sys.argv`, then calls the same
 * `vibe.cli.entrypoint.main` the real launcher calls. Argv is assigned after
 * the interpreter is already running, so it never goes through execve() and
 * stays outside ARG_MAX (issue #1009).
 *
 * Also sidesteps a bug in vibe's own `get_prompt_from_stdin()`
 * (vibe/cli/cli.py): it reads a piped prompt, then reopens `/dev/tty` to
 * restore interactive stdin, which raises OSError with no controlling
 * terminal (as under this server) and discards the prompt it just read. This
 * bridge drains stdin first, so that call sees EOF and returns None — our
 * sys.argv prompt wins instead. Confirmed against the installed CLI. */
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
    // vibe's CLI has no model/effort flag; the session's pin (if any) has
    // nowhere to go here, unlike agyBackendSpec below.
    turnArgs: () => [resolveInterpreter(), '-c', VIBE_STDIN_BRIDGE],
    encodeStdin: (prompt) => prompt,
    decodeToolRequest: decodeVibeToolRequest
  };
}

/** Bounded servicing budget per turn (#1041): every decoded request costs
 * one more backend invocation, and an unbounded loop would let a backend
 * that only ever emits tool requests spin the turn forever. */
const MAX_TOOL_ROUNDS_PER_TURN = 5;
const MAX_TOOL_OUTPUT_BYTES = 256 * 1024;

/** The wire shape vibe's print mode leaks when its model emits a tool call
 * the CLI didn't execute (#1041): `<name>\u{C8F0}<json args>` -- one Unicode
 * separator (U+C8F0) between the tool name and the JSON argument object,
 * observed live as
 * `read_file\u{C8F0}{"file_path": ".../memoryGatewayClient.ts"}`. Returns
 * null for an ordinary reply; throws when the shape IS present but cannot
 * be decoded, so the turn ends with an actionable error rather than
 * surfacing raw tool syntax as a successful assistant reply. */
export function decodeVibeToolRequest(reply: string): HeadlessToolRequest | null {
  const raw = reply.trim();
  const match = /^(\S+?)\u{C8F0}(\{[\s\S]*\})$/u.exec(raw);
  if (!match) return null;
  const name = match[1];
  let args: unknown;
  try {
    args = JSON.parse(match[2]);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`Vibe emitted a tool request for "${name}" whose arguments are not valid JSON (${detail}); refusing to surface raw tool syntax as a reply.`);
  }
  return { name, args: args as Record<string, unknown>, raw };
}

interface HeadlessToolPlan {
  title: string;
  kind: string;
  locations: string[];
  run: () => Promise<string>;
}

function isInside(child: string, parent: string): boolean {
  return child === parent || child.startsWith(parent + path.sep);
}

/** getcwd() may hand the backend child a symlink-resolved cwd (macOS
 * /var -> /private/var), and the emitted file_path is absolute, so
 * containment is judged on canonical paths -- falling back to the lexical
 * form only for a target that does not exist on disk yet. A target that
 * realpath-resolves somewhere new (a symlink escape) is judged on its
 * canonical form alone, never its lexical one. */
function isInsideWorkingDirectory(resolved: string, cwd: string): boolean {
  const canonical = (target: string): string => {
    try {
      return realpathSync(target);
    } catch {
      return target;
    }
  };
  const realCwd = canonical(cwd);
  const realResolved = canonical(resolved);
  if (realResolved !== resolved) return isInside(realResolved, realCwd);
  return isInside(resolved, cwd) || isInside(resolved, realCwd);
}

/** GAH-side executor for decoded tool requests (#1041). The request names
 * the BACKEND's tool vocabulary, so only the read-only subset GAH can
 * safely execute itself (a bounded repository read) is servable; anything
 * else fails the turn with an actionable error. Reads are confined to the
 * conversation's working directory -- the session worktree a headless turn
 * already runs in. */
function planHeadlessTool(displayName: string, name: string, args: Record<string, unknown>, cwd: string): HeadlessToolPlan {
  if (name !== 'read_file') {
    throw new Error(`${displayName} requested tool "${name}", which GAH cannot execute on the backend's behalf. Executable tools: read_file.`);
  }
  const requested = args.file_path;
  if (typeof requested !== 'string' || requested.trim().length === 0) {
    throw new Error(`${displayName} requested read_file without a valid "file_path" argument.`);
  }
  const resolved = path.resolve(cwd, requested);
  if (!isInsideWorkingDirectory(resolved, cwd)) {
    throw new Error(`${displayName} requested to read "${requested}", which resolves outside this conversation's working directory (${cwd}).`);
  }
  return {
    title: `Read ${resolved}`,
    kind: 'read',
    locations: [resolved],
    run: async () => {
      let content: Buffer;
      try {
        content = readFileSync(resolved);
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new Error(`${displayName} requested to read "${resolved}" but the read failed: ${detail}`);
      }
      return content.length > MAX_TOOL_OUTPUT_BYTES
        ? `${content.subarray(0, MAX_TOOL_OUTPUT_BYTES).toString('utf8')}\n[GAH truncated the read at ${MAX_TOOL_OUTPUT_BYTES} bytes]`
        : content.toString('utf8');
    }
  };
}

/** Mirrors acpAdapter's card digest: first 5 lines, 400 chars. */
function summarizeOutput(text: string): string {
  return text.split('\n').slice(0, 5).join('\n').slice(0, 400);
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
 * the session cwd.
 *
 * #1032 reopened: a session pinned to a model/effort was displayed as such
 * but silently launched agy with its own default (model="", whichever pool
 * `agy` itself falls back to) because turnArgs never carried the pin. The
 * installed CLI takes both natively as `--model`/`--effort`; append them
 * only when the session actually pinned one, so an unpinned session's argv
 * is unchanged. */
export function agyBackendSpec(): HeadlessBackendSpec {
  return {
    id: 'agy',
    displayName: 'Agy',
    turnArgs: (opts = {}) => {
      const args = [
        'agy',
        '--input-format',
        'stream-json',
        '--output-format',
        'stream-json',
        '--dangerously-skip-permissions',
        '--sandbox'
      ];
      if (opts.model) args.push('--model', opts.model);
      if (opts.reasoningEffort) args.push('--effort', opts.reasoningEffort);
      return args;
    },
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
