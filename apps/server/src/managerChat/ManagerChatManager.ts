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

// Serializes turns per profile -- without this, two concurrent messages for
// the same profile (e.g. two open browser tabs) would both prompt the same
// backend session at once, corrupting turn ordering. One profile = one
// conversation, so turns must run one at a time.
const turnQueueByProfile = new Map<string, Promise<unknown>>();
const activeProfiles = new Set<string>();

// Live tee of assistant/chunk log writes (#959): the session log is the
// record, and the WebSocket layer pushes a copy of each chunk to every
// subscribed client so a turn renders progressively. Registered by
// wsServer.ts (setChunkPublisher). Kept a hook rather than importing the
// push bus directly so ManagerChatManager stays transport-agnostic and
// tests can observe chunks without a socket.
type ChunkPublish = (chunk: {
  type: 'manager.chat.chunk';
  requestId: string;
  profile: string;
  turn: number;
  seq: number;
  text: string;
}) => void;
let chunkPublisher: ChunkPublish | undefined;

export function setChunkPublisher(publish: ChunkPublish | undefined): void {
  chunkPublisher = publish;
}

export function getChunkPublisher(): ChunkPublish | undefined {
  return chunkPublisher;
}

/** Mutable state for the one in-flight turn per profile, so a cancel can
 * close the writer, sequence its turn/end, and skip capture without racing
 * the closure's own locals (#960). */
interface ActiveTurn {
  requestId: string;
  turnNo: number;
  seq: number;
  chunkWriter: ReturnType<typeof createEventWriter> | undefined;
  cancelled: boolean;
  /** Resolves the moment cancelManagerChatTurn runs, so the in-flight turn
   * can race the backend's acknowledgement against a settle deadline. */
  cancelSettled: Promise<void>;
  resolveCancel: () => void;
}
const activeTurns = new Map<string, ActiveTurn>();

const CANCEL_SETTLE_TIMEOUT_MS = 8_000;

/** Session log storage options (tests may point at a temp state dir). */
const logOptions: SessionLogOptions = {};

export function setSessionLogOptions(opts: SessionLogOptions): void {
  logOptions.stateDir = opts.stateDir;
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

export async function runTurn(
  profile: string,
  message: string,
  prompt: string,
  history: ChatTranscriptTurn[],
  onChunk: (text: string) => void,
  onToolResult: (name: string, text: string) => void,
  active: ActiveTurn
): Promise<{ reply: string; backend: string; model: string | null; usage: ChatUsage | null }> {
  const backendId = backendForProfile(profile);
  const adapter = resolveAdapter(backendId);
  const isSlashCommand = message.trim().startsWith('/');
  const result = await adapter.runTurn(profile, { prompt, history, onChunk, onToolResult });

  // A cancelled turn is closed as cancelled, never captured as a completed
  // exchange (a partial reply must not enter the memory gateway).
  if (active.cancelled) {
    return { reply: result.reply, backend: backendId, model: result.model, usage: result.usage };
  }
  await capture(profile, message, result.reply);
  // Force buffered L0 conversation into the gateway's L1/L2 pipeline right
  // away on a compact/clear-like command, instead of waiting for the
  // pipeline's own idle timeout (#849).
  if (isSlashCommand && isCompactionCommand(message)) {
    await flushSession(profile);
  }
  return { reply: result.reply, backend: backendId, model: result.model, usage: result.usage };
}

export interface ManagerChatTurnResult {
  turn: ChatTranscriptTurn;
  cancelled: boolean;
}

/** Cancels the in-flight turn for a profile, if one is running. Safe to call
 * when nothing is in flight (returns false) -- never appends a spurious
 * turn/end in that case (#960). The ACP session itself survives; only the
 * current prompt turn is stopped. */
export async function cancelManagerChatTurn(profile: string): Promise<boolean> {
  const active = activeTurns.get(profile);
  if (!active) return false;
  active.cancelled = true;
  active.resolveCancel();
  const backendId = backendForProfile(profile);
  try {
    await resolveAdapter(backendId).cancelTurn(profile);
  } catch (error) {
    console.error(`[managerChat] cancel failed for profile ${profile}:`, error);
  }
  return true;
}

export function sendManagerChatMessage(profile: string, message: string, requestId?: string): Promise<ManagerChatTurnResult> {
  const prior = turnQueueByProfile.get(profile) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async (): Promise<ManagerChatTurnResult> => {
    const existing = loadLog(profile, logOptions);
    const history = deriveModelHistory(existing);
    const turnNo = existing.reduce((highest, event) => Math.max(highest, event.turn), 0) + 1;
    const now = Date.now();
    const compaction = isCompactionCommand(message);
    let resolveCancel!: () => void;
    const active: ActiveTurn = {
      requestId: requestId ?? '',
      turnNo,
      seq: existing.reduce((highest, event) => Math.max(highest, event.seq), 0),
      chunkWriter: undefined,
      cancelled: false,
      cancelSettled: new Promise<void>((resolve) => { resolveCancel = resolve; }),
      resolveCancel: () => resolveCancel()
    };
    activeTurns.set(profile, active);
    activeProfiles.add(profile);
    appendEvents(profile, [
      ...(compaction ? [{ type: 'compaction/start' as const, seq: ++active.seq, turn: turnNo, timestamp: now }] : []),
      { type: 'turn/start', seq: ++active.seq, turn: turnNo, timestamp: now },
      { type: 'user/message', seq: ++active.seq, turn: turnNo, text: message, source: 'prompt', timestamp: now }
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
          seq: ++active.seq,
          turn: turnNo,
          text: prompt,
          source: 'inject',
          timestamp: Date.now()
        }], logOptions);
      }
      active.chunkWriter = createEventWriter(profile, logOptions);
      const run = runTurn(
        profile,
        message,
        prompt,
        history,
        (text) => {
          const chunk = {
            type: 'assistant/chunk' as const,
            seq: ++active.seq,
            turn: turnNo,
            text,
            timestamp: Date.now()
          };
          active.chunkWriter?.append(chunk);
          if (chunkPublisher) {
            chunkPublisher({
              type: 'manager.chat.chunk',
              requestId: requestId ?? '',
              profile,
              turn: turnNo,
              seq: chunk.seq,
              text
            });
          }
        },
        (name, text) => active.chunkWriter?.append({
          type: 'tool/result',
          seq: ++active.seq,
          turn: turnNo,
          name,
          text,
          timestamp: Date.now()
        }),
        active
      );
      // A cancel is a barrier for the queue: once we've sent session/cancel
      // to the backend, don't let an unresponsive agent wedge the profile's
      // turn queue forever. Real agents reply with stopReason 'cancelled';
      // this races a settle deadline only for the pathological case.
      const settleDeadline = active.cancelSettled.then(
        () => new Promise<never>((_resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('cancel timed out waiting for backend to stop')), CANCEL_SETTLE_TIMEOUT_MS);
          timer.unref();
        })
      );
      const result = await Promise.race([run, settleDeadline]);
      const { reply, backend, model, usage } = result;
      await active.chunkWriter.close();
      const assistant: ChatTranscriptTurn = {
        role: 'assistant',
        text: reply,
        backend,
        model,
        usage,
        timestamp: Date.now()
      };
      const done: ChatSessionEvent[] = active.cancelled
        ? [{ type: 'turn/end', seq: ++active.seq, turn: turnNo, reason: { kind: 'cancelled' }, timestamp: Date.now() }]
        : [
            {
              type: 'assistant/message',
              seq: ++active.seq,
              turn: turnNo,
              text: reply,
              backend,
              model,
              usage,
              timestamp: assistant.timestamp
            },
            ...(isSlashCommand ? [{
              type: 'human/command' as const,
              seq: ++active.seq,
              turn: turnNo,
              command: message.trim().slice(1).split(/\s+/)[0] ?? '',
              result: reply,
              timestamp: Date.now()
            }] : []),
            { type: 'turn/end', seq: ++active.seq, turn: turnNo, reason: { kind: 'complete' }, timestamp: Date.now() },
            ...(compaction ? [{
              type: 'compaction/summary' as const,
              seq: ++active.seq,
              turn: turnNo,
              summary: compactionSummary(message, reply),
              timestamp: Date.now()
            }] : []),
            ...(compaction ? [{ type: 'compaction/end' as const, seq: ++active.seq, turn: turnNo, timestamp: Date.now() }] : [])
          ];
      appendEvents(profile, done, logOptions);
      return { turn: assistant, cancelled: active.cancelled };
    } catch (error) {
      await active.chunkWriter?.close().catch(() => undefined);
      const text = error instanceof Error ? error.message : String(error);
      const done: ChatSessionEvent[] = [
        { type: 'harness/error', seq: ++active.seq, turn: turnNo, text, timestamp: Date.now() },
        { type: 'turn/end', seq: ++active.seq, turn: turnNo, reason: { kind: 'error', message: text }, timestamp: Date.now() }
      ];
      appendEvents(profile, done, logOptions);
      throw error;
    } finally {
      activeProfiles.delete(profile);
      activeTurns.delete(profile);
    }
  });
  turnQueueByProfile.set(profile, turn.catch(() => undefined));
  return turn;
}
