/**
 * Orchestrates one manager-chat conversation per GAH profile: recall memory
 * context, run a turn against the profile's configured backend (resuming
 * its session if one exists), capture the exchange back to memory. One
 * profile = one ongoing conversation per backend, matching "talk to the
 * manager for this repo" from the UI.
 */

import { recall, capture, flushSession } from './memoryGatewayClient.js';
import { resolveAdapter } from './registry.js';
import { backendForProfile } from './settingsStore.js';

export interface ChatTurn {
  role: 'user' | 'assistant' | 'system';
  text: string;
  timestamp: number;
}

export interface ChatTurnResult {
  reply: string;
  cleared?: boolean;
}

const MAX_HISTORY_PER_PROFILE = 200;

// Keyed by `${profile}::${backendId}` -- switching a profile's backend must
// not try to resume a different backend's session id.
const sessionIdByProfileBackend = new Map<string, string>();
const historyByProfile = new Map<string, ChatTurn[]>();
// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both read the same
// resumeSessionId and race to spawn the backend CLI, corrupting session
// state. One profile = one conversation, so turns must run one at a time.
const turnQueueByProfile = new Map<string, Promise<unknown>>();

function appendHistory(profile: string, turn: ChatTurn): void {
  const history = historyByProfile.get(profile) ?? [];
  history.push(turn);
  if (history.length > MAX_HISTORY_PER_PROFILE) {
    history.splice(0, history.length - MAX_HISTORY_PER_PROFILE);
  }
  historyByProfile.set(profile, history);
}

export function getHistory(profile: string): ChatTurn[] {
  return historyByProfile.get(profile) ?? [];
}

async function runClear(profile: string, backendId: string): Promise<ChatTurnResult> {
  const flushed = await flushSession(profile);
  sessionIdByProfileBackend.delete(`${profile}::${backendId}`);
  historyByProfile.set(profile, []);
  const reply = flushed
    ? 'Chat cleared. Memory flushed and the conversation session was reset.'
    : 'Chat cleared and the conversation session was reset, but flushing memory failed -- see server logs.';
  return { reply, cleared: true };
}

async function runCompact(profile: string, backendId: string): Promise<ChatTurnResult> {
  // Flush now (forces the gateway's L0->L1/L2 extraction immediately rather
  // than waiting for its own idle timeout) then drop the live session id so
  // the next turn starts a fresh, cheaper backend session. The visible
  // transcript stays -- only the underlying session resets -- and recall()
  // re-surfaces the compacted summary on the next turn as needed.
  const flushed = await flushSession(profile);
  sessionIdByProfileBackend.delete(`${profile}::${backendId}`);
  const reply = flushed
    ? 'Context compacted: memory flushed and the live session reset. Recent history stays visible; older context is now summarized in memory.'
    : 'Tried to compact, but flushing memory failed -- see server logs. The live session was still reset.';
  return { reply };
}

async function runTurn(profile: string, message: string): Promise<ChatTurnResult> {
  const backendId = backendForProfile(profile);
  const trimmed = message.trim().toLowerCase();

  if (trimmed === '/clear') {
    return runClear(profile, backendId);
  }
  if (trimmed === '/compact') {
    return runCompact(profile, backendId);
  }

  const adapter = resolveAdapter(backendId);
  const sessionKey = `${profile}::${backendId}`;

  const { context } = await recall(profile, message);
  const prompt = context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;

  const { sessionId, reply } = await adapter.runTurn(prompt, sessionIdByProfileBackend.get(sessionKey));
  sessionIdByProfileBackend.set(sessionKey, sessionId);

  await capture(profile, message, reply);
  return { reply };
}

export function sendManagerChatMessage(profile: string, message: string): Promise<ChatTurnResult> {
  appendHistory(profile, { role: 'user', text: message, timestamp: Date.now() });

  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async () => {
    try {
      const result = await runTurn(profile, message);
      if (!result.cleared) {
        appendHistory(profile, { role: 'assistant', text: result.reply, timestamp: Date.now() });
      }
      return result;
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      appendHistory(profile, { role: 'system', text: `Error: ${text}`, timestamp: Date.now() });
      throw error;
    }
  });
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
