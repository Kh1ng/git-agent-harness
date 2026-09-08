/** A remote agent implements the same adapter as a local agent. Only the
 * registered worker chooses paths and launches processes; central keeps history. */
import { randomUUID } from 'node:crypto';
import { nodeHeaders, type RegistryService } from './registryService.js';
import type { ManagerAdapter } from './managerChat/registry.js';
import { parseWorkerChatEvent, readWorkerChatReply } from './workerChatProtocol.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';

export function workerChatConnection(registry: RegistryService, nodeId: string, profile: string, project: Pick<ProfileSummary, 'repo' | 'provider' | 'web_url'>) {
  if (!project.web_url) throw new Error('Project provider URL is missing. Refresh its import before using worker chat.');
  const origin = new URL(project.web_url).origin;
  const node = registry.getNode(nodeId);
  if (!node || !node.profiles?.includes(profile)) throw new Error('Worker is not registered for this project.');
  const endpoint = new URL('/api/worker-chat', node.advertised_url);
  if (endpoint.protocol !== 'https:' && process.env.GAH_ALLOW_INSECURE_HTTP !== '1'
    && !['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)) {
    throw new Error('Worker chat requires HTTPS or explicit trusted LAN HTTP configuration.');
  }
  const assertRegistration = () => {
    if (registry.getNode(nodeId) !== node) throw new Error('Worker registration changed. Select the worker again.');
  };
  const post = async (body: Record<string, unknown>, signal: AbortSignal) => {
    assertRegistration();
    const response = await fetch(endpoint, {
      method: 'POST', redirect: 'error', signal,
      headers: { ...nodeHeaders(node), 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...body, profile, repo: project.repo, provider: project.provider, origin, nodeId })
    });
    assertRegistration();
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(`Worker chat request failed (${response.status}). Check this node's profile and backend readiness.`);
    }
    return response;
  };
  return {
    async request<T>(body: Record<string, unknown>): Promise<T> {
      const response = await post(body, AbortSignal.timeout(30_000));
      const result = await readWorkerChatReply(response, { ...body, profile });
      assertRegistration();
      return result as T;
    },
    adapter(backend: string, sessionId?: string): ManagerAdapter {
      let active: { requestId: string; abort: AbortController } | undefined;
      const command = <T>(action: string, extra: Record<string, unknown> = {}) => this.request<T>({ action, backend, sessionId, ...extra });
      return {
        id: backend, displayName: `${backend} on ${node.display_name}`, implemented: true,
        async runTurn(_key, input) {
          if (active) throw new Error('Worker adapter is already serving a turn.');
          const requestId = randomUUID();
          const abort = new AbortController();
          active = { requestId, abort };
          const timeout = setTimeout(() => abort.abort(), 60 * 60_000);
          timeout.unref?.();
          const unsubscribe = registry.onChange(() => {
            if (registry.getNode(nodeId) !== node) abort.abort();
          });
          let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
          try {
            const response = await post({ action: 'run', requestId, backend, sessionId, prompt: input.prompt, history: input.history, model: input.model, reasoningEffort: input.reasoningEffort }, abort.signal);
            if (!response.headers.get('content-type')?.startsWith('application/x-ndjson') || !response.body) throw new Error('Worker does not support chat streaming. Update the worker.');
            reader = response.body.getReader();
            const decoder = new TextDecoder();
            let buffer = '';
            while (true) {
              const part = await reader.read();
              buffer += decoder.decode(part.value, { stream: !part.done });
              if (buffer.length > 2_000_000) throw new Error('Worker chat event exceeds the supported size.');
              let end: number;
              while ((end = buffer.indexOf('\n')) >= 0) {
                const event = parseWorkerChatEvent(JSON.parse(buffer.slice(0, end)));
                buffer = buffer.slice(end + 1);
                assertRegistration();
                if (event.type === 'chunk') input.onChunk(event.text);
                else if (event.type === 'toolResult') input.onToolResult(event.name, event.text);
                else if (event.type === 'toolCall') input.onToolCall?.(event.tool);
                else if (event.type === 'permission') {
                  // Keep consuming the stream while the operator decides; Stop must
                  // remain responsive and the worker disconnect must close the turn.
                  void Promise.resolve(input.requestPermission?.(event.request) ?? 'cancelled').then(optionId =>
                    command('permission', { requestId, permissionId: event.id, optionId })
                  ).catch(() => abort.abort());
                } else if (event.type === 'result') return event.result;
                else if (event.type === 'error') throw new Error(event.code === 'usage_limit'
                  ? 'Worker agent usage limit reached.'
                  : 'Worker agent failed or stopped. Check its backend readiness.');
              }
              if (part.done) throw new Error('Worker disconnected before completing the turn.');
            }
          } finally {
            clearTimeout(timeout);
            unsubscribe();
            await reader?.cancel().catch(() => undefined);
            abort.abort();
            active = undefined;
          }
        },
        listCommands: () => command('commands'),
        listModels: () => command('models'),
        setModel: async () => { throw new Error('Set the model on the conversation before the next worker turn.'); },
        setReasoningEffort: async () => { throw new Error('Set reasoning effort on the conversation before the next worker turn.'); },
        steerTurn: async (_key, message) => {
          if (!active) throw new Error('No worker turn is active.');
          await command('steer', { requestId: active.requestId, message });
          return { outcome: 'injected' };
        },
        cancelTurn: async () => {
          if (!active) return;
          const turn = active;
          try { await command('cancel', { requestId: turn.requestId }); }
          finally { turn.abort.abort(); }
        }
      };
    }
  };
}
