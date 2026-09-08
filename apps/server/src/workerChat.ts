/** Worker-side agent execution. Paths come from local profiles, never requests;
 * central owns prompts, permissions, memory, skills, and the conversation log. */
import { Router } from 'express';
import { randomUUID } from 'node:crypto';
import type { ChatSessionSummary, ChatTranscriptTurn, NodeRoleStatus, ProfileSummary } from '@git-agent-harness/contracts';
import { runProfileList } from './gahCli.js';
import { resolveAdapter, type ManagerAdapter } from './managerChat/registry.js';
import { archiveSession, chatKey, createSession, getSession, resolveSessionCwd, touchSession, updateSession, type ChatSessionStoreOptions } from './managerChat/chatSessions.js';

type TurnInput = Parameters<ManagerAdapter['runTurn']>[1];
export type WorkerChatEvent =
  | { type: 'chunk'; text: string }
  | { type: 'toolResult'; name: string; text: string }
  | { type: 'toolCall'; tool: Parameters<NonNullable<TurnInput['onToolCall']>>[0] }
  | { type: 'permission'; id: string; request: Parameters<NonNullable<TurnInput['requestPermission']>>[0] }
  | { type: 'result'; result: Awaited<ReturnType<ManagerAdapter['runTurn']>> }
  | { type: 'error'; error: string };

interface ActiveExecution {
  key: string;
  adapter: ManagerAdapter;
  permission?: { id: string; choices: Set<string>; resolve: (choice: string) => void };
  stop: () => void;
}

export function createWorkerChatRouter(deps: {
  node: NodeRoleStatus;
  profiles?: () => Promise<ProfileSummary[]>;
  adapter?: typeof resolveAdapter;
  sessions?: ChatSessionStoreOptions;
}) {
  const router = Router();
  const active = new Map<string, ActiveExecution>();
  const profiles = deps.profiles ?? runProfileList;
  const adapterFor = deps.adapter ?? resolveAdapter;
  router.post('/', async (req, res) => {
    if (deps.node.role !== 'worker') return void res.status(409).json({ error: 'Agent execution belongs on a worker.' });
    const body = req.body;
    if (!body || typeof body.profile !== 'string' || !body.profile || typeof body.action !== 'string') {
      return void res.status(400).json({ error: 'A profile and worker chat action are required.' });
    }
    const actions = ['create', 'prepare', 'archive', 'models', 'commands', 'run', 'cancel', 'steer', 'permission'];
    if (!actions.includes(body.action)) return void res.status(400).json({ error: 'Unknown worker chat action.' });
    const sessionId = body.sessionId;
    if (sessionId !== undefined && (typeof sessionId !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(sessionId))) {
      return void res.status(400).json({ error: 'Invalid chat session identity.' });
    }
    try {
      const profile = (await profiles()).find(candidate => candidate.name === body.profile);
      if (!profile) return void res.status(404).json({ error: 'This worker does not have that profile.' });
      if (body.repo !== undefined && body.repo !== profile.repo) return void res.status(409).json({ error: 'The worker profile refers to a different repository.' });
      const key = chatKey(body.profile, sessionId);
      if (['create', 'prepare', 'archive', 'run'].includes(body.action) && [...active.values()].some(turn => turn.key === key)) {
        return void res.status(409).json({ error: 'Stop the active turn before changing this worker workspace.' });
      }
      if (body.action === 'run' && (typeof body.requestId !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(body.requestId)
        || typeof body.prompt !== 'string' || !Array.isArray(body.history)
        || body.history.length > 2000 || !body.history.every((turn: ChatTranscriptTurn) => turn && ['user', 'assistant', 'system', 'tool'].includes(turn.role) && typeof turn.text === 'string'))) {
        return void res.status(400).json({ error: 'Invalid worker turn.' });
      }
      if (['cancel', 'steer', 'permission'].includes(body.action)) {
        const execution = active.get(body.requestId);
        if (!execution || execution.key !== key) return void res.status(409).json({ error: 'No matching worker turn is active.' });
        if (body.action === 'cancel') {
          execution.stop();
          void execution.adapter.cancelTurn(key).catch(() => undefined);
        } else if (body.action === 'steer') {
          if (typeof body.message !== 'string' || !body.message.trim()) return void res.status(400).json({ error: 'A steer message is required.' });
          await execution.adapter.steerTurn(key, body.message);
        } else {
          const pending = execution.permission;
          if (!pending || pending.id !== body.permissionId || !pending.choices.has(body.optionId)) return void res.status(409).json({ error: 'Permission is no longer available or the choice is invalid.' });
          execution.permission = undefined;
          pending.resolve(body.optionId);
        }
        return void res.json({ success: true });
      }
      const backend = body.backend;
      if (typeof backend !== 'string') return void res.status(400).json({ error: 'A backend is required.' });
      const adapter = adapterFor(backend);
      if (body.action === 'models') return void res.json(await adapter.listModels(key));
      if (body.action === 'commands') return void res.json(await adapter.listCommands(key));
      const settings = {
        backend,
        model: typeof body.model === 'string' ? body.model : null,
        reasoningEffort: typeof body.reasoningEffort === 'string' ? body.reasoningEffort : null,
        ...(typeof body.title === 'string' ? { title: body.title } : {})
      };
      if (body.action === 'create') {
        return void res.status(201).json(await createSession({ profile: body.profile, profileInfo: profile, ...settings }, deps.sessions));
      }
      if (body.action === 'archive') {
        if (!sessionId) return void res.status(400).json({ error: 'A session is required.' });
        if ([...active.values()].some(turn => turn.key === key)) return void res.status(409).json({ error: 'Stop the active turn before archiving its workspace.' });
        return void res.json(await archiveSession(body.profile, sessionId, profile, deps.sessions));
      }
      let session: ChatSessionSummary | null = null;
      let cwd = profile.local_path;
      if (sessionId) {
        if (!getSession(body.profile, sessionId, deps.sessions)) {
          await createSession({ profile: body.profile, profileInfo: profile, ...settings, sessionId }, deps.sessions);
        }
        updateSession(body.profile, sessionId, settings, deps.sessions);
        const resolved = await resolveSessionCwd(body.profile, sessionId, profile, deps.sessions);
        if (!resolved) return void res.status(409).json({ error: 'Worker session is unavailable or archived.' });
        session = resolved.session;
        cwd = resolved.cwd;
      }
      if (body.action === 'prepare') return void res.json({ session });
      if (active.has(body.requestId) || [...active.values()].some(turn => turn.key === key)) return void res.status(409).json({ error: 'This worker conversation already has an active turn.' });
      let stop!: () => void;
      const stopped = new Promise<never>((_, reject) => { stop = () => reject(new Error('Worker turn stopped.')); });
      const execution: ActiveExecution = { key, adapter, stop: () => { execution.permission?.resolve('cancelled'); stop(); } };
      active.set(body.requestId, execution);
      res.status(200).set({ 'Content-Type': 'application/x-ndjson', 'Cache-Control': 'no-store', 'X-Accel-Buffering': 'no' });
      res.flushHeaders();
      const emit = (event: WorkerChatEvent) => {
        if (res.writableLength > 2_000_000) {
          execution.stop();
          void adapter.cancelTurn(key).catch(() => undefined);
          return;
        }
        if (!res.destroyed && !res.writableEnded) res.write(`${JSON.stringify(event)}\n`);
      };
      const disconnect = () => {
        if (res.writableEnded) return;
        execution.stop();
        void adapter.cancelTurn(key).catch(() => undefined);
      };
      res.once('close', disconnect);
      try {
        const result = await Promise.race([stopped, adapter.runTurn(key, {
          prompt: body.prompt, history: body.history, cwd, model: settings.model, reasoningEffort: settings.reasoningEffort,
          onChunk: text => emit({ type: 'chunk', text }),
          onToolResult: (name, text) => emit({ type: 'toolResult', name, text }),
          onToolCall: tool => emit({ type: 'toolCall', tool }),
          requestPermission: request => new Promise<string>(resolve => {
            const id = randomUUID();
            execution.permission = { id, choices: new Set(request.options.map(option => option.optionId)), resolve };
            emit({ type: 'permission', id, request });
          })
        })]);
        if (sessionId) touchSession(body.profile, sessionId, deps.sessions);
        emit({ type: 'result', result });
      } catch {
        emit({ type: 'error', error: 'Worker agent failed or the turn was stopped. Check the worker backend readiness.' });
      } finally {
        execution.permission?.resolve('cancelled');
        active.delete(body.requestId);
        res.off('close', disconnect);
        res.end();
      }
    } catch {
      if (!res.headersSent) res.status(502).json({ error: 'Worker chat operation failed. Check the profile and backend on this worker.' });
    }
  });
  return router;
}
