/**
 * Orchestrates one manager-chat conversation per GAH profile: recall memory
 * context, run a turn against the profile's configured backend, capture the
 * exchange back to memory. Slash commands (Hermes's real /reset, /compress,
 * etc.) are sent through like any other message -- the backend adapter's
 * own session dispatches them natively, GAH doesn't reinvent them.
 */

import { recall, capture, flushSession } from './memoryGatewayClient.js';
import { resolveAdapter, type ManagerCommandInfo, type ManagerModelInfo } from './registry.js';
import { backendForProfile } from './settingsStore.js';

// Backend-native command names that mean "compact/clear this session"
// (Hermes: /reset, /compress; Codex/Claude ACP bridges: /compact, /clear).
// GAH doesn't reinvent these commands (see runTurn below), so it can't rely
// on a single canonical name -- it just recognizes the common synonyms well
// enough to know when to also flush buffered memory (#849).
const COMPACTION_COMMAND_NAMES = new Set(['compact', 'compress', 'clear', 'reset']);

export function isCompactionCommand(message: string): boolean {
  const name = message.trim().slice(1).split(/\s+/)[0]?.toLowerCase();
  return name !== undefined && COMPACTION_COMMAND_NAMES.has(name);
}

export interface ChatTurn {
  role: 'user' | 'assistant' | 'system';
  text: string;
  timestamp: number;
}

const MAX_HISTORY_PER_PROFILE = 200;

const historyByProfile = new Map<string, ChatTurn[]>();
// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both prompt the same
// backend session at once, corrupting turn ordering. One profile = one
// conversation, so turns must run one at a time.
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

export function listCommandsForProfile(profile: string): Promise<ManagerCommandInfo[]> {
  const backendId = backendForProfile(profile);
  return resolveAdapter(backendId).listCommands(profile);
}

export function listModelsForProfile(profile: string): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }> {
  const backendId = backendForProfile(profile);
  return resolveAdapter(backendId).listModels(profile);
}

export function setModelForProfile(profile: string, modelId: string): Promise<void> {
  const backendId = backendForProfile(profile);
  return resolveAdapter(backendId).setModel(profile, modelId);
}

async function runTurn(profile: string, message: string): Promise<string> {
  const backendId = backendForProfile(profile);
  const adapter = resolveAdapter(backendId);

  // Slash commands (Hermes's real /reset, /compress, etc.) must reach the
  // backend verbatim -- its command parser only recognizes a message that
  // *starts* with "/". Wrapping it in a "Relevant context..." prefix (as
  // every other message gets) silently turns a real command into a plain
  // question the model tries to answer instead of dispatching. Confirmed
  // empirically: a wrapped "/reset" got interpreted as chat text ("What
  // would you like me to reset?"); the bare message dispatched correctly
  // ("Conversation history cleared.").
  const isSlashCommand = message.trim().startsWith('/');
  const prompt = isSlashCommand ? message : await (async () => {
    const { context } = await recall(profile, message);
    return context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;
  })();

  const { reply } = await adapter.runTurn(profile, prompt);

  await capture(profile, message, reply);
  // Force buffered L0 conversation into the gateway's L1/L2 pipeline right
  // away on a compact/clear-like command, instead of waiting for the
  // pipeline's own idle timeout (#849).
  if (isSlashCommand && isCompactionCommand(message)) {
    await flushSession(profile);
  }
  return reply;
}

export function sendManagerChatMessage(profile: string, message: string): Promise<string> {
  appendHistory(profile, { role: 'user', text: message, timestamp: Date.now() });

  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async () => {
    try {
      const reply = await runTurn(profile, message);
      appendHistory(profile, { role: 'assistant', text: reply, timestamp: Date.now() });
      return reply;
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      appendHistory(profile, { role: 'system', text: `Error: ${text}`, timestamp: Date.now() });
      throw error;
    }
  });
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
