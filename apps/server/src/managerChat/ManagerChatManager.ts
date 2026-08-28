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
import { resolveAdapter, listManagerBackends, type ManagerCommandInfo, type ManagerModelInfo } from './registry.js';
import { compactionSummary, isCompactionCommand, isUsageLimitError } from './acpAdapter.js';
import { backendForProfile, modelOverrideForProfile, setModelOverrideForProfile } from './settingsStore.js';
import { effectiveContextPolicy, applyContextBudget } from '../gatewaySettingsStore.js';
import { appendEvents, createEventWriter, deriveModelHistory, foldSession, loadLog, type SessionLogOptions } from './sessionLog.js';
import {
  archiveSession,
  chatKey,
  chatSessionStoreOptions,
  createSession,
  getSession,
  listSessions,
  resolveSessionCwd,
  touchSession,
  type ChatSessionStoreOptions
} from './chatSessions.js';
import { runProfileList } from '../gahCli.js';
import type { ChatSessionEvent, ChatTranscriptTurn, ChatUsage } from '@git-agent-harness/contracts';
import type { ProfileSummary } from '@git-agent-harness/contracts';

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

/** Chat session store options (tests may point at a temp state dir). */
export function setChatSessionStoreOptions(opts: ChatSessionStoreOptions): void {
  chatSessionStoreOptions.stateDir = opts.stateDir;
}

let profileInfoCache: { at: number; profiles: ProfileSummary[] } | null = null;
const PROFILE_INFO_TTL_MS = 30_000;

/** Resolves a profile's config facts (repo_id, local_path, worktree_base)
 * for session worktree management. Cached briefly: profiles are hot-path
 * enough (every session turn) but change rarely, and `gah profile list`
 * shells out to the Rust CLI. */
async function findProfileInfo(profile: string): Promise<ProfileSummary | null> {
  const now = Date.now();
  if (!profileInfoCache || now - profileInfoCache.at > PROFILE_INFO_TTL_MS) {
    profileInfoCache = { at: now, profiles: await runProfileList() };
  }
  return profileInfoCache.profiles.find((p) => p.name === profile) ?? null;
}

/** The full folded view, including cursor + streaming state. */
export function getSessionView(profile: string, sessionId?: string) {
  const opts = sessionId ? { ...logOptions, sessionId } : logOptions;
  return foldSession(profile, opts, !activeProfiles.has(chatKey(profile, sessionId)));
}

/** Lists a profile's chat sessions (WP2). */
export function listChatSessions(profile: string) {
  return listSessions(profile, chatSessionStoreOptions);
}

/** Creates a chat session bound to a fresh worktree (WP2). The backend
 * resolves at create time: explicit request, else the profile default. */
export async function createChatSession(profile: string, backend?: string, title?: string) {
  const profileInfo = await findProfileInfo(profile);
  if (!profileInfo) throw new Error(`Profile '${profile}' not found`);
  return createSession(
    { profile, profileInfo, backend: backend ?? backendForProfile(profile), title },
    chatSessionStoreOptions
  );
}

/** Archives a chat session: dirty worktree patched first, branch survives (WP2). */
export async function archiveChatSession(profile: string, sessionId: string) {
  const profileInfo = await findProfileInfo(profile);
  if (!profileInfo) throw new Error(`Profile '${profile}' not found`);
  return archiveSession(profile, sessionId, profileInfo, chatSessionStoreOptions);
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

interface HandoffInfo {
  from: string;
  to: string;
  reason: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Pure handoff orchestration (#962), factored out so unit tests can drive
 * it with mock attempt functions instead of real backend processes. */
export interface HandoffAttemptInput {
  startBackend: string;
  fallbackBackends: string[];
  attempt: (backendId: string) => Promise<{ reply: string; model: string | null; usage: ChatUsage | null }>;
}

export async function handoffAttempt(
  input: HandoffAttemptInput
): Promise<{ reply: string; backend: string; model: string | null; usage: ChatUsage | null; handoff: HandoffInfo | null }> {
  const { startBackend, fallbackBackends, attempt } = input;
  let backend = startBackend;
  let handoff: HandoffInfo | null = null;
  try {
    const result = await attempt(startBackend);
    return { ...result, backend, handoff };
  } catch (error) {
    // Only a usage/quota limit triggers an automatic handoff. Any other
    // failure (auth, backend crash, network) surfaces as today's error.
    if (!isUsageLimitError(error)) throw error;

    for (const fallback of fallbackBackends) {
      try {
        const result = await attempt(fallback);
        backend = fallback;
        handoff = { from: startBackend, to: fallback, reason: errorMessage(error) };
        return { ...result, backend, handoff };
      } catch (fallbackError) {
        // At most one automatic handoff per turn: a second limit error fails
        // the turn normally (AC4), surfacing the exhausted-model error.
        if (isUsageLimitError(fallbackError)) throw error;
        // Non-limit fallback failure (e.g. not installed) -> try the next.
        continue;
      }
    }
    // No eligible fallback: today's behavior -- fail with the limit error.
    throw error;
  }
}

/** The result type `runTurn` produces after any handoff. */
export type TurnRunResult = {
  reply: string;
  backend: string;
  model: string | null;
  usage: ChatUsage | null;
  handoff: HandoffInfo | null;
};

export interface RunTurnContext {
  /** Composite key identifying this conversation (profile or profile#session). */
  key: string;
  /** Backend serving this conversation; the session's own backend for
   * session-bound turns, the profile default otherwise. */
  backend: string;
  /** Session working directory (WP2); undefined = the server's cwd. */
  cwd?: string;
}

export async function runTurn(
  profile: string,
  message: string,
  prompt: string,
  history: ChatTranscriptTurn[],
  onChunk: (text: string) => void,
  onToolResult: (name: string, text: string) => void,
  active: ActiveTurn,
  context: RunTurnContext
): Promise<TurnRunResult> {
  const isSlashCommand = message.trim().startsWith('/');
  // #962: flush the session to the gateway before the handoff so the new
  // backend's recall sees the pre-handoff exchange. Best-effort: a flush
  // failure must not block the handoff itself. The flush is per-PROFILE --
  // project memory is shared across a profile's sessions by design.
  const result = await handoffAttempt({
    startBackend: context.backend,
    fallbackBackends: listManagerBackends().filter((b) => b.implemented && b.id !== context.backend).map((b) => b.id),
    attempt: async (backendId) => {
      const adapter = resolveAdapter(backendId);
      const attempt = async () => {
        const r = await adapter.runTurn(context.key, { prompt, history, onChunk, onToolResult, cwd: context.cwd });
        return r;
      };
      try {
        return await attempt();
      } catch (error) {
        if (isUsageLimitError(error)) {
          await flushSession(profile).catch(() => undefined);
        }
        throw error;
      }
    }
  });

  // A cancelled turn is closed as cancelled, never captured as a completed
  // exchange (a partial reply must not enter the memory gateway).
  if (!active.cancelled) {
    const captured = await capture(profile, message, result.reply);
    // #878: a failing gateway never hard-blocks the turn, but the skipped
    // capture must be visible in the transcript, not silent.
    if (captured.degraded) {
      appendEvents(profile, [{
        type: 'harness/error',
        seq: ++active.seq,
        turn: active.turnNo,
        text: `memory gateway degraded (capture skipped, memory may be lost): ${captured.error ?? 'unknown error'}`,
        timestamp: Date.now()
      }], logOptions);
    }
    // Force buffered L0 conversation into the gateway's L1/L2 pipeline right
    // away on a compact/clear-like command, instead of waiting for the
    // pipeline's own idle timeout (#849).
    if (isSlashCommand && isCompactionCommand(message)) {
      await flushSession(profile);
    }
  }
  return result;
}

export interface ManagerChatTurnResult {
  turn: ChatTranscriptTurn;
  cancelled: boolean;
}

/** Cancels the in-flight turn for a conversation, if one is running. Safe to
 * call when nothing is in flight (returns false) -- never appends a spurious
 * turn/end in that case (#960). The ACP session itself survives; only the
 * current prompt turn is stopped. */
export async function cancelManagerChatTurn(profile: string, sessionId?: string): Promise<boolean> {
  const key = chatKey(profile, sessionId);
  const active = activeTurns.get(key);
  if (!active) return false;
  active.cancelled = true;
  active.resolveCancel();
  const backendId = (sessionId ? getSession(profile, sessionId, chatSessionStoreOptions)?.backend : undefined)
    ?? backendForProfile(profile);
  try {
    await resolveAdapter(backendId).cancelTurn(key);
  } catch (error) {
    console.error(`[managerChat] cancel failed for ${key}:`, error);
  }
  return true;
}

export function sendManagerChatMessage(profile: string, message: string, requestId?: string, sessionId?: string): Promise<ManagerChatTurnResult> {
  // Session-bound turns (WP2): resolve the session's worktree cwd (re-
  // materializing it from the branch if prune reclaimed the idle worktree)
  // and serve the turn from the session's own backend. Unknown or archived
  // sessions fail loudly rather than silently landing in the default log.
  const prepareSession = async (): Promise<{ cwd?: string; backend: string }> => {
    if (!sessionId || sessionId === 'default') {
      return { backend: backendForProfile(profile) };
    }
    const profileInfo = await findProfileInfo(profile);
    if (!profileInfo) {
      throw new Error(`Profile '${profile}' not found for chat session '${sessionId}'`);
    }
    const resolved = await resolveSessionCwd(profile, sessionId, profileInfo, chatSessionStoreOptions);
    if (!resolved) {
      throw new Error(`No active chat session '${sessionId}' for profile '${profile}'`);
    }
    return { cwd: resolved.cwd, backend: resolved.session.backend };
  };

  const key = chatKey(profile, sessionId);
  const sessionOpts = sessionId && sessionId !== 'default' ? { ...logOptions, sessionId } : logOptions;  const prior = turnQueueByProfile.get(key) ?? Promise.resolve();
  const turn = prior.catch(() => undefined).then(async (): Promise<ManagerChatTurnResult> => {
    const sessionContext = await prepareSession();
    const existing = loadLog(profile, sessionOpts);
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
    activeTurns.set(key, active);
    activeProfiles.add(key);
    appendEvents(profile, [
      ...(compaction ? [{ type: 'compaction/start' as const, seq: ++active.seq, turn: turnNo, timestamp: now }] : []),
      { type: 'turn/start', seq: ++active.seq, turn: turnNo, timestamp: now },
      { type: 'user/message', seq: ++active.seq, turn: turnNo, text: message, source: 'prompt', timestamp: now }
    ], sessionOpts);

    try {
      // Keep slash commands bare so the backend dispatches them instead of
      // sending them to the model as ordinary text.
      const isSlashCommand = message.trim().startsWith('/');
      let injectPolicy: { budgetChars?: number; tiers?: string[] } | undefined;
      let truncated = false;
      let context = '';
      if (!isSlashCommand) {
        const policy = effectiveContextPolicy(profile);
        const recalled = await recall(profile, message);
        // #878: a failing gateway never hard-blocks the turn, but the skipped
        // recall context must be visible in the transcript, not silent.
        if (recalled.degraded) {
          appendEvents(profile, [{
            type: 'harness/error',
            seq: ++active.seq,
            turn: turnNo,
            text: `memory gateway degraded (recall context skipped): ${recalled.error ?? 'unknown error'}`,
            timestamp: Date.now()
          }], logOptions);
        }
        // #961: budget injected recall deterministically (highest-relevance
        // head first), and record the policy + truncation on the inject
        // event so a replay explains itself.
        const budgeted = applyContextBudget(recalled.context, policy);
        context = budgeted.text;
        truncated = budgeted.truncated;
        injectPolicy = {
          ...(policy.budgetChars ? { budgetChars: policy.budgetChars } : {}),
          ...(policy.tiers && policy.tiers.length > 0 ? { tiers: policy.tiers } : {})
        };
        if (truncated) {
          // Never silently: tell the agent the recall was cut and that more
          // exists, so it knows to ask for the rest instead of trusting the
          // slice as complete. (The on-demand path is /api/context/recall.)
          context += '\n\n[Note: recall was truncated to the context budget; additional memory exists. Say "recall more context about <topic>" to fetch it.]';
        }
      }
      const prompt = context ? `Relevant context from prior conversations:\n${context}\n\nUser: ${message}` : message;
      if (context) {
        appendEvents(profile, [{
          type: 'user/message',
          seq: ++active.seq,
          turn: turnNo,
          text: prompt,
          source: 'inject',
          timestamp: Date.now(),
          policy: injectPolicy,
          truncated
        }], sessionOpts);
      }
      active.chunkWriter = createEventWriter(profile, sessionOpts);
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
              ...(sessionId ? { sessionId } : {}),
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
        active,
        { key, backend: sessionContext.backend, cwd: sessionContext.cwd }
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
      const { reply, backend, model, usage, handoff } = result;
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
            ...(handoff ? [{
              type: 'handoff' as const,
              seq: ++active.seq,
              turn: turnNo,
              from: handoff.from,
              fromModel: model ?? null,
              to: handoff.to,
              toModel: null,
              reason: handoff.reason,
              timestamp: Date.now()
            }] : []),
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
      appendEvents(profile, done, sessionOpts);
      return { turn: assistant, cancelled: active.cancelled };
    } catch (error) {
      await active.chunkWriter?.close().catch(() => undefined);
      const text = error instanceof Error ? error.message : String(error);
      const done: ChatSessionEvent[] = [
        { type: 'harness/error', seq: ++active.seq, turn: turnNo, text, timestamp: Date.now() },
        { type: 'turn/end', seq: ++active.seq, turn: turnNo, reason: { kind: 'error', message: text }, timestamp: Date.now() }
      ];
      appendEvents(profile, done, sessionOpts);
      throw error;
    } finally {
      if (sessionId && sessionId !== 'default') touchSession(profile, sessionId, chatSessionStoreOptions);
      activeProfiles.delete(key);
      activeTurns.delete(key);
    }
  });
  turnQueueByProfile.set(key, turn.catch(() => undefined));
  return turn;
}
