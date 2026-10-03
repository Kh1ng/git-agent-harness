import { findGahBinary, type BackendInstanceRuntime } from '../gahCli.js';
import type { SpawnSpec } from './acpAdapter.js';
import type { HeadlessBackendSpec } from './headlessAdapter.js';
import type { ManagerAdapter } from './registry.js';

/** Launch named instances through the local CLI. The CLI resolves the secret
 * before changing HOME, then applies the runner's isolated child environment.
 * Only public instance metadata crosses the server/CLI boundary. */
function instanceArgs(profile: string, runtime: BackendInstanceRuntime, model?: string | null): string[] {
  const args = ['config', 'exec-backend-instance', '--profile', profile, '--instance', runtime.backend_instance];
  if (model) args.push('--model', model);
  return args;
}

function ownerEnvironment(env?: Record<string, string>): Record<string, string> | undefined {
  if (!env) return undefined;
  // Outer GAH must retain the owner's HOME to find local credentials. These
  // paths belong to its eventual child and are set by Rust after lookup.
  return Object.fromEntries(Object.entries(env).filter(([name]) =>
    !['HOME', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'HERMES_HOME', 'VIBE_HOME',
      'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME'].includes(name)));
}

export function instanceAcpSpawn(profile: string, runtime: BackendInstanceRuntime, spec: SpawnSpec): SpawnSpec {
  const args = instanceArgs(profile, runtime);
  if (runtime.runner_kind === 'codex' || runtime.runner_kind === 'claude') {
    args.push('--acp-bridge', spec.args[0]);
    args.push('--', ...spec.args.slice(1));
  } else {
    args.push('--', ...spec.args);
  }
  return { command: findGahBinary(), args, env: ownerEnvironment(spec.env) };
}

export function instanceHeadlessSpec(profile: string, runtime: BackendInstanceRuntime, spec: HeadlessBackendSpec): HeadlessBackendSpec {
  return {
    ...spec,
    turnArgs: (options) => {
      const original = spec.turnArgs(options);
      const args = instanceArgs(profile, runtime, options?.model);
      if (runtime.runner_kind === 'vibe') args.push('--adapter-program', original[0]);
      return [findGahBinary(), ...args, '--', ...original.slice(1)];
    },
    ...(spec.spawnEnv ? { spawnEnv: async (gahProfile: string) => ownerEnvironment(await spec.spawnEnv!(gahProfile)) ?? {} } : {})
  };
}

function canonicalProvider(provider: string): string {
  return ({ 'nous-portal': 'nous', moonshot: 'kimi', 'x-ai': 'xai', gemini: 'google' } as Record<string, string>)[provider] ?? provider;
}

/** OpenCode model IDs identify their credential provider. Hermes and
 * OpenHands IDs identify model vendors instead, so this guard is OpenCode-only. */
export function bindOpenCodeModels(adapter: ManagerAdapter, runtime: BackendInstanceRuntime): ManagerAdapter {
  if (runtime.runner_kind !== 'opencode' || !runtime.credential_id || !runtime.credential_provider) return adapter;
  const provider = canonicalProvider(runtime.credential_provider);
  const allowed = (model: string) => model.includes('/') && canonicalProvider(model.split('/')[0]) === provider;
  const requireModel = (model?: string | null) => {
    if (model && !allowed(model)) throw new Error('That model uses a different provider than this instance’s selected credential.');
  };
  return {
    ...adapter,
    async listModels(profile, cwd) {
      const result = await adapter.listModels(profile, cwd);
      return { ...result, models: result.models.filter(model => allowed(model.id)),
        currentModelId: result.currentModelId && allowed(result.currentModelId) ? result.currentModelId : null };
    },
    async setModel(profile, model) {
      requireModel(model);
      const result = await adapter.listModels(profile);
      if (!result.models.some(candidate => candidate.id === model && allowed(candidate.id))) {
        throw new Error('That model is no longer available for this instance’s selected credential.');
      }
      await adapter.setModel(profile, model);
    },
    async runTurn(profile, input) {
      requireModel(input.model);
      const result = await adapter.listModels(profile, input.cwd);
      let model = input.model;
      if (model && !result.models.some(candidate => candidate.id === model && allowed(candidate.id))) {
        throw new Error('That model is no longer available for this instance’s selected credential.');
      }
      if (!model) {
        model = result.currentModelId && allowed(result.currentModelId)
          ? result.currentModelId : result.models.find(candidate => allowed(candidate.id))?.id;
        if (!model) throw new Error('This instance has no model for its selected credential provider.');
        await adapter.setModel(profile, model);
      }
      return adapter.runTurn(profile, { ...input, model });
    }
  };
}
