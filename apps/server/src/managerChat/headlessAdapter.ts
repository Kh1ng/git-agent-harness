/**
 * Headless (Tier B) manager-chat adapter (slice 4).
 *
 * For backends with no ACP or equivalent structured protocol (vibe, agy,
 * openhands): each turn is ONE non-interactive CLI invocation (`--print`-
 * style). Conversation context is reconstructed textually per turn — the
 * same replay approach the ACP path uses when a backend forgets its
 * history (see resumePrompt in acpAdapter.ts): full transcript replay for
 * a fresh process, since a one-shot process has no memory at all.
 *
 * What this buys: every backend in the unified chat surface, session
 * worktree binding (cwd per conversation), backend interchange, quota
 * handoff, and the event-sourced log — everything except streaming,
 * native slash commands, per-backend model listing, and the permission
 * round-trip, which need a structured protocol (upgrade to Tier A later:
 * opencode already has native ACP; agy has a stream-json mode worth
 * exploring).
 */

import { spawn } from 'node:child_process';
import type { ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';
import type { ManagerAdapter, ManagerCommandInfo, ManagerModelInfo } from './registry.js';

export type { ManagerCommandInfo, ManagerModelInfo };

export interface HeadlessSpawnSpec {
  command: string;
  args: string[];
  /** Env for the child process. */
  env?: Record<string, string>;
}

/** Builds the CLI argv for one turn: prompt and cwd are passed by the
 * engine; the spec adds backend-specific flags. */
export interface HeadlessBackendSpec {
  id: string;
  displayName: string;
  /** argv builder for one print-mode turn. */
  turnArgs: (prompt: string) => string[];
  /** Parse the CLI's stdout into the reply text. Default: trimmed stdout. */
  parseReply?: (stdout: string) => string;
  /** Per-conversation in-memory transcript state (for historyDelta). */
  // (kept by the engine, not the spec)
}

interface ConversationState {
  knownHistory: ChatTranscriptTurn[];
  /** Process handle for the in-flight turn (cancel support). */
  child: ReturnType<typeof spawn> | null;
}

const conversations = new Map<string, ConversationState>();

/** Mirrors acpAdapter's resumePrompt: fresh process => replay the whole
 * conversation as text before the new prompt. */
function replayPrompt(message: string, history: ChatTranscriptTurn[]): string {
  if (history.length === 0) return message;
  const lines = history.map((turn) => `${turn.role}: ${turn.text}`);
  return `${lines.join('\n')}\n\nuser: ${message}`;
}

const TURN_TIMEOUT_MS = 10 * 60_000;

export function createHeadlessBackend(spec: HeadlessBackendSpec): ManagerAdapter {
  const states = new Map<string, ConversationState>();

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

      const args = spec.turnArgs(prompt);
      const child = spawn(args[0], args.slice(1), {
        cwd,
        env: { ...process.env },
        stdio: ['ignore', 'pipe', 'pipe']
      });
      state.child = child;

      let stdout = '';
      let stderr = '';
      child.stdout.on('data', (chunk) => { stdout += chunk; });
      child.stderr.on('data', (chunk) => { stderr += chunk; });

      const killTimer = setTimeout(() => {
        child.kill('SIGKILL');
      }, TURN_TIMEOUT_MS);
      killTimer.unref?.();

      try {
        const code = await new Promise<number | null>((resolve, reject) => {
          child.on('error', reject);
          child.on('close', (exitCode) => resolve(exitCode));
        });
        if (code !== 0) {
          const detail = stderr.trim().slice(0, 400) || `exit code ${code}`;
          throw new Error(`${spec.displayName} turn failed: ${detail}`);
        }
        const reply = (spec.parseReply ?? ((out: string) => out.trim()))(stdout);
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

    async listModels(): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }> {
      return { models: [], currentModelId: null }; // no model picker protocol
    },

    async setModel(): Promise<void> {
      throw new Error(`${spec.displayName} doesn't support model selection in headless mode.`);
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

/** vibe: Mistral's CLI, print mode. */
export function vibeBackendSpec(): HeadlessBackendSpec {
  return {
    id: 'vibe',
    displayName: 'Vibe',
    turnArgs: (prompt) => ['vibe', '-p', prompt, '--output', 'text']
  };
}

/** agy: print mode with JSON output for a clean reply parse. */
export function agyBackendSpec(): HeadlessBackendSpec {
  return {
    id: 'agy',
    displayName: 'Agy',
    turnArgs: (prompt) => ['agy', '--print', '--output-format', 'text', prompt]
  };
}
