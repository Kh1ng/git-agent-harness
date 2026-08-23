/**
 * Orchestrates one manager-chat conversation per GAH profile: recall memory
 * context, run a turn against the profile's configured backend, capture the
 * exchange back to memory. Slash commands (Hermes's real /reset, /compress,
 * etc.) are sent through like any other message -- the backend adapter's
 * own session dispatches them natively, GAH doesn't reinvent them.
 *
 * Issue #955: conversation history is an event-sourced session log (see
 * sessionLog.ts) rather than an in-memory array -- it survives server
 * restart, derives the transcript by folding, and carries per-message
 * backend/model/usage attribution.
 */

import { recall, capture, flushSession } from './memoryGatewayClient.js';
import { resolveAdapter, type ManagerCommandInfo, type ManagerModelInfo } from './registry.js';
import { backendForProfile, modelOverrideForProfile, setModelOverrideForProfile } from './settingsStore.js';
import { appendEvents, foldSession, type SessionLogOptions } from './sessionLog.js';
import type { ChatSessionEvent, ChatTranscriptTurn } from '@git-agent-harness/contracts';

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

/** The derived transcript turn -- the on-wire shape of a folded log. */
export type ChatTurn = ChatTranscriptTurn;

// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both prompt the same
// backend session at once, corrupting turn ordering. One profile = one
// conversation, so turns must run one at a time.
const turnQueueByProfile = new Map<string, Promise<unknown>>();

/** Session log storage options (tests may point at a temp state dir). */
const logOptions: SessionLogOptions = {};

export function setSessionLogOptions(opts: SessionLogOptions): void {
  logOptions.stateDir = opts.stateDir;
}

/** Fold the event log into the derived transcript (survives restart). */
export function getHistory(profile: string): ChatTurn[] {
  return foldSession(profile, logOptions).turns;
}

/** The full folded view, including cursor + streaming state. */
export function getSessionView(profile: string) {
  return foldSession(profile, logOptions);
}

export function listCommandsForProfile(profile: string): Promise<ManagerCommandInfo[]> {
  const backendId = backendForProfile(profile);
  return resolveAdapter(backendId).listCommands(profile);
}

// The ACP connection itself only remembers the current model in memory
// (ProfileConnection.currentModelId) -- if that connection is ever recreated
// (backend crash, quota error, server restart), a fresh session reverts to
// the backend's own default and the user's choice is silently lost. Restore
// a persisted override here, right after fetching whatever the (possibly
// fresh) connection reports as current, rather than trusting the connection
// to remember across its own lifetime.
export async function listModelsForProfile(
  profile: string
): Promise<{ models: ManagerModelInfo[]; currentModelId: string | null }> {
  const backendId = backendForProfile(profile);
  const adapter = resolveAdapter(backendId);
  const { models, currentModelId } = await adapter.listModels(profile);
  const override = modelOverrideForProfile(profile, backendId);
  if (override && override !== currentModelId && models.some((m) => m.id === override)) {
    await adapter.setModel(profile, override);
    return { models, currentModelId: override };
  }
  return { models, currentModelId };
}

export async function setModelForProfile(profile: string, modelId: string): Promise<void> {
  const backendId = backendForProfile(profile);
  await resolveAdapter(backendId).setModel(profile, modelId);
  setModelOverrideForProfile(profile, backendId, modelId);
}

let nextSeq = 1;
/** Monotonic per-process seq. Persisted events keep their assigned seq. */
function nextEventSeq(): number {
  return nextSeq++;
}

async function runTurn(profile: string, message: string): Promise<{ reply: string; model: string | null }> {
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

  const result = await adapter.runTurn(profile, prompt);

  await capture(profile, message, result.reply);
  // Force buffered L0 conversation into the gateway's L1/L2 pipeline right
  // away on a compact/clear-like command, instead of waiting for the
  // pipeline's own idle timeout (#849).
  if (isSlashCommand && isCompactionCommand(message)) {
    await flushSession(profile);
  }
  return { reply: result.reply, model: result.model };
}

export function sendManagerChatMessage(profile: string, message: string): Promise<string> {
  const now = Date.now();
  const turnNo = foldSession(profile, logOptions).turns.length + 1;

  const events: ChatSessionEvent[] = [
    { type: 'turn/start', seq: nextEventSeq(), turn: turnNo, timestamp: now },
    { type: 'user/message', seq: nextEventSeq(), turn: turnNo, text: message, source: 'prompt', timestamp: now }
  ];
  appendEvents(profile, events, logOptions);

  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async () => {
    try {
      const { reply, model } = await runTurn(profile, message);
      const backendId = backendForProfile(profile);
      const done: ChatSessionEvent[] = [
        {
          type: 'assistant/message',
          seq: nextEventSeq(),
          turn: turnNo,
          text: reply,
          backend: backendId,
          model,
          usage: null,
          timestamp: Date.now()
        },
        { type: 'turn/end', seq: nextEventSeq(), turn: turnNo, reason: { kind: 'complete' }, timestamp: Date.now() }
      ];
      appendEvents(profile, done, logOptions);
      return reply;
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      const done: ChatSessionEvent[] = [
        { type: 'tool/result', seq: nextEventSeq(), turn: turnNo, name: 'error', text, timestamp: Date.now() },
        { type: 'turn/end', seq: nextEventSeq(), turn: turnNo, reason: { kind: 'error', message: text }, timestamp: Date.now() }
      ];
      appendEvents(profile, done, logOptions);
      throw error;
    }
  });
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
