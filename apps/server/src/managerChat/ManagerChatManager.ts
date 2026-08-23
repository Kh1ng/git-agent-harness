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
import { compactionSummary, isCompactionCommand } from './acpAdapter.js';
import { backendForProfile, modelOverrideForProfile, setModelOverrideForProfile } from './settingsStore.js';
import { appendEvents, createEventWriter, deriveModelHistory, foldSession, loadLog, type SessionLogOptions } from './sessionLog.js';
import type { ChatSessionEvent, ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';

export { compactionSummary, isCompactionCommand } from './acpAdapter.js';

/** The derived transcript turn -- the on-wire shape of a folded log. */
export type ChatTurn = ChatTranscriptTurn;

// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both prompt the same
// backend session at once, corrupting turn ordering. One profile = one
// conversation, so turns must run one at a time.
const turnQueueByProfile = new Map<string, Promise<unknown>>();
const activeProfiles = new Set<string>();

/** Session log storage options (tests may point at a temp state dir). */
const logOptions: SessionLogOptions = {};

export function setSessionLogOptions(opts: SessionLogOptions): void {
  logOptions.stateDir = opts.stateDir;
}

/** Fold the event log into the derived transcript (survives restart). */
export function getHistory(profile: string): ChatTurn[] {
  return foldSession(profile, logOptions, !activeProfiles.has(profile)).turns;
}

/** The full folded view, including cursor + streaming state. */
export function getSessionView(profile: string) {
  return foldSession(profile, logOptions, !activeProfiles.has(profile));
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

async function runTurn(
  profile: string,
  message: string,
  prompt: string,
  history: ChatTranscriptTurn[],
  onChunk: (text: string) => void
): Promise<{ reply: string; backend: string; model: string | null; usage: ChatUsage | null }> {
  const backendId = backendForProfile(profile);
  const adapter = resolveAdapter(backendId);
  const isSlashCommand = message.trim().startsWith('/');
  const result = await adapter.runTurn(profile, { prompt, history, onChunk });

  await capture(profile, message, result.reply);
  // Force buffered L0 conversation into the gateway's L1/L2 pipeline right
  // away on a compact/clear-like command, instead of waiting for the
  // pipeline's own idle timeout (#849).
  if (isSlashCommand && isCompactionCommand(message)) {
    await flushSession(profile);
  }
  return { reply: result.reply, backend: backendId, model: result.model, usage: result.usage };
}

export function sendManagerChatMessage(profile: string, message: string): Promise<ChatTranscriptTurn> {
  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async () => {
    const existing = loadLog(profile, logOptions);
    const history = deriveModelHistory(existing);
    let seq = existing.reduce((highest, event) => Math.max(highest, event.seq), 0);
    const turnNo = existing.reduce((highest, event) => Math.max(highest, event.turn), 0) + 1;
    const now = Date.now();
    const compaction = isCompactionCommand(message);
    let chunkWriter: ReturnType<typeof createEventWriter> | undefined;
    activeProfiles.add(profile);
    appendEvents(profile, [
      ...(compaction ? [{ type: 'compaction/start' as const, seq: ++seq, turn: turnNo, timestamp: now }] : []),
      { type: 'turn/start', seq: ++seq, turn: turnNo, timestamp: now },
      { type: 'user/message', seq: ++seq, turn: turnNo, text: message, source: 'prompt', timestamp: now }
    ], logOptions);

    try {
      // Keep slash commands bare so the backend dispatches them instead of
      // sending them to the model as ordinary text.
      const isSlashCommand = message.trim().startsWith('/');
      const { context } = isSlashCommand ? { context: '' } : await recall(profile, message);
      const prompt = context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;
      if (context) {
        appendEvents(profile, [{
          type: 'user/message',
          seq: ++seq,
          turn: turnNo,
          text: prompt,
          source: 'inject',
          timestamp: Date.now()
        }], logOptions);
      }
      chunkWriter = createEventWriter(profile, logOptions);
      const { reply, backend, model, usage } = await runTurn(profile, message, prompt, history, (text) => {
        chunkWriter?.append({
          type: 'assistant/chunk',
          seq: ++seq,
          turn: turnNo,
          text,
          timestamp: Date.now()
        });
      });
      await chunkWriter.close();
      const assistant: ChatTranscriptTurn = {
        role: 'assistant',
        text: reply,
        backend,
        model,
        usage,
        timestamp: Date.now()
      };
      const done: ChatSessionEvent[] = [
        {
          type: 'assistant/message',
          seq: ++seq,
          turn: turnNo,
          text: reply,
          backend,
          model,
          usage,
          timestamp: assistant.timestamp
        },
        { type: 'turn/end', seq: ++seq, turn: turnNo, reason: { kind: 'complete' }, timestamp: Date.now() },
        ...(compaction ? [{
          type: 'compaction/summary' as const,
          seq: ++seq,
          turn: turnNo,
          summary: compactionSummary(message, reply),
          timestamp: Date.now()
        }] : []),
        ...(compaction ? [{ type: 'compaction/end' as const, seq: ++seq, turn: turnNo, timestamp: Date.now() }] : [])
      ];
      appendEvents(profile, done, logOptions);
      return assistant;
    } catch (error) {
      await chunkWriter?.close().catch(() => undefined);
      const text = error instanceof Error ? error.message : String(error);
      const done: ChatSessionEvent[] = [
        { type: 'tool/result', seq: ++seq, turn: turnNo, name: 'error', text, timestamp: Date.now() },
        { type: 'turn/end', seq: ++seq, turn: turnNo, reason: { kind: 'error', message: text }, timestamp: Date.now() }
      ];
      appendEvents(profile, done, logOptions);
      throw error;
    } finally {
      activeProfiles.delete(profile);
    }
  });
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
