import { useEffect, useRef, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import { gahApi } from '../api/client.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { AgentModelSelect, type AgentModelOption } from './AgentModelSelect.js';
import { agentDisplayName } from './LiveAgentsCard.js';
import { currentAgentModelLabel } from '../lib/agentModelLabel.js';
import { agentModelOptions, taskReasoningEfforts } from '../lib/agentModelOptions.js';

const INPUT_CLASS = 'min-h-11 bg-raised border border-subtle rounded-md px-3 py-2 text-sm text-primary';
const EFFORT_LABELS: Record<string, string> = { low: 'Low', medium: 'Medium', high: 'High', xhigh: 'Extra high', max: 'Maximum', ultra: 'Ultra' };

/** `codex/gpt-5` as its backend and model; a model may itself contain `/`. */
function splitAgent(key: string): { backend: string; model: string } {
  const slash = key.indexOf('/');
  return slash < 0 ? { backend: key, model: '' } : { backend: key.slice(0, slash), model: key.slice(slash + 1) };
}

/** `codex/gpt-5` reads "Codex · gpt-5": the subscription, then the exact model. */
export function agentLabel(key: string): string {
  const { backend, model } = splitAgent(key);
  return model ? `${agentDisplayName(backend)} · ${model}` : agentDisplayName(backend);
}

/** A whole number of at least 1, or undefined for blank or invalid text. */
function count(text: string): number | undefined {
  const value = Number(text.trim());
  return text.trim() !== '' && Number.isInteger(value) && value >= 1 ? value : undefined;
}

/**
 * The models each backend's own CLI offers right now, as the names dispatch
 * passes to it: Antigravity is addressed by display name, the others by id.
 * A backend that lists nothing just gets no suggestions.
 */
export function useBackendModels(profile: string, backends: string[]): Record<string, { options: AgentModelOption[]; efforts?: { id: string; name: string }[]; loading: boolean; failed: boolean }> {
  const [models, setModels] = useState<Record<string, { options: AgentModelOption[]; efforts?: { id: string; name: string }[]; loading: boolean; failed: boolean }>>({});
  const wanted = backends.join(',');
  useEffect(() => {
    let cancelled = false;
    setModels(Object.fromEntries((wanted ? wanted.split(',') : []).map((backend) => [backend, { options: [], loading: true, failed: false }])));
    const aliases = wanted.split(',').some((backend) => /^claude(?:[:_-]|$)/i.test(backend))
      ? gahApi.getRoleMetrics(profile, '30d').then((report) => report.model_aliases ?? []).catch(() => [])
      : Promise.resolve([]);
    for (const backend of wanted ? wanted.split(',') : []) {
      Promise.all([gahApi.getManagerChatModelsForBackend(profile, backend), aliases])
        .then(([summary, resolvedAliases]) => {
          const options = agentModelOptions(backend, summary.models, resolvedAliases);
          if (!cancelled) setModels((current) => ({ ...current, [backend]: { options, efforts: summary.reasoningEfforts, loading: false, failed: false } }));
        })
        .catch(() => { if (!cancelled) setModels((current) => ({ ...current, [backend]: { options: [], loading: false, failed: true } })); });
    }
    return () => { cancelled = true; };
  }, [profile, wanted]);
  return models;
}

/**
 * Each agent's model and the baseline worker counts: how many jobs run at
 * once in total, and how many of them each agent may take. `agents` lists
 * every `backend/model` in the profile's routing, capped or not.
 */
export function AgentLimitsSection({ selectedName, selected, agents, onSaved }: {
  selectedName: string;
  selected: Pick<ProfileSummary, 'max_parallel_workers' | 'max_concurrent_per_model' | 'agent_reasoning_effort'>;
  agents: string[];
  /** A model switch changes the routing lists the page shows elsewhere. */
  onSaved?: () => void;
}) {
  const updateProfile = useGahStore((s) => s.updateProfile);
  const saving = useGahStore((s) => s.profileCrud.updating);
  const saveError = useGahStore((s) => s.profileCrud.updateError);
  const fetchStatus = useGahStore((s) => s.fetchStatus);

  const caps = selected.max_concurrent_per_model;
  const keys = [...new Set([...agents, ...Object.keys(caps ?? {})])].sort();
  const suggestions = useBackendModels(selectedName, [...new Set(agents.map((key) => splitAgent(key).backend))].sort());
  const [total, setTotal] = useState('');
  const [limits, setLimits] = useState<Record<string, string>>({});
  /** Edited model per agent key; an agent absent here keeps its model. */
  const [models, setModels] = useState<Record<string, string>>({});
  const [efforts, setEfforts] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState(false);
  // Seed once per profile so a refresh after saving keeps in-progress edits.
  const seededProfileRef = useRef<string | null>(null);
  useEffect(() => {
    if (seededProfileRef.current === selectedName) return;
    seededProfileRef.current = selectedName;
    setTotal(selected.max_parallel_workers != null ? String(selected.max_parallel_workers) : '');
    setLimits(Object.fromEntries(Object.entries(caps ?? {}).map(([key, value]) => [key, String(value)])));
    setModels({});
    setEfforts({});
  }, [selectedName, selected.max_parallel_workers, caps]);

  if (!caps) {
    return (
      <section className="card-padded">
        <h3 className="text-sm font-semibold text-primary mb-1">Models & worker limits</h3>
        <p className="text-xs text-muted">This node's <code>gah</code> does not report per-agent limits yet. Update it to edit them here.</p>
      </section>
    );
  }

  const switches = keys.flatMap((key) => {
    const { backend, model } = splitAgent(key);
    const next = (models[key] ?? model).trim();
    return agents.includes(key) && next !== model ? [{ key, backend, from: model, to: next }] : [];
  });
  const invalid = (total.trim() !== '' && count(total) === undefined)
    || keys.some((key) => (limits[key] ?? '').trim() !== '' && count(limits[key]) === undefined);
  const modelError = switches.some((change) => change.to === '')
    ? 'An agent needs a model name.'
    : new Set(keys.map((key) => { const change = switches.find((candidate) => candidate.key === key); return change ? `${change.backend}/${change.to}` : key; })).size !== keys.length
      ? 'That subscription already has an agent on that model.'
      : null;
  const save = async () => {
    setSaved(false);
    // The command switches models first and the cap moves with the model, so limits are sent under the new name.
    const renamed = (key: string) => {
      const change = switches.find((candidate) => candidate.key === key);
      return change ? `${change.backend}/${change.to}` : key;
    };
    await updateProfile(selectedName, {
      ...(count(total) !== undefined ? { max_parallel_workers: count(total) } : {}),
      agent_effort: Object.entries(efforts).map(([backend, effort]) => `${backend}=${effort}`),
      agent_model: switches.map((change) => `${change.backend}/${change.from}=${change.to}`),
      // A blank agent sends 0, which removes its cap. Nothing is cleared wholesale, so a
      // server that does not know this field yet leaves the saved caps alone.
      max_concurrent: keys.filter((key) => count(limits[key] ?? '') !== undefined || key in caps).map((key) => `${renamed(key)}=${count(limits[key] ?? '') ?? 0}`),
    });
    if (useGahStore.getState().profileCrud.updateError) return;
    // The saved profile names the agents differently now: seed again from it.
    seededProfileRef.current = null;
    await fetchStatus(selectedName, { force: true });
    onSaved?.();
    setSaved(true);
  };

  return (
    <section className="card-padded" aria-labelledby="agent-limits-title">
      <h3 id="agent-limits-title" className="text-sm font-semibold text-primary mb-1">Models, reasoning & worker limits</h3>
      <p className="text-xs text-muted mb-3">
        Choose a model and reasoning amount for each account, then set how many jobs can run at once.
      </p>
      <div className="flex items-center justify-between gap-3 border-b border-subtle pb-2">
        <label htmlFor="agent-limits-total" className="text-sm font-medium text-primary">Base worker capacity</label>
        <input id="agent-limits-total" type="number" min={1} value={total} onChange={(e) => setTotal(e.target.value)} placeholder="1" className={`w-24 tabular-nums ${INPUT_CLASS}`} />
      </div>
      {keys.length === 0 ? (
        <p className="mt-2 text-xs text-muted">No models are configured yet. Open Advanced routing to add an agent.</p>
      ) : (
        <ul className="divide-y divide-subtle">
          {keys.map((key) => {
            const { backend, model } = splitAgent(key);
            const value = models[key] ?? model;
            const group = suggestions[backend]?.options.find((option) => option.variants?.some((variant) => variant.value === value));
            const nativeEffort = /^(codex|claude)$/.test(backend) && selected.agent_reasoning_effort !== undefined;
            const reasoning = group?.variants?.map((variant) => ({ id: variant.effort, name: EFFORT_LABELS[variant.effort] ?? variant.effort }))
              ?? (nativeEffort ? [{ id: 'default', name: 'Provider default' }, ...taskReasoningEfforts(backend, suggestions[backend]?.efforts).filter((effort) => effort.id !== 'default').map((effort) => ({ ...effort, name: EFFORT_LABELS[effort.id] ?? effort.name }))] : []);
            const effortValue = group?.variants?.find((variant) => variant.value === value)?.effort ?? efforts[backend] ?? selected.agent_reasoning_effort?.[backend] ?? 'default';
            return (
              <li key={key} className="grid grid-cols-1 items-start gap-3 py-4 sm:grid-cols-[minmax(0,1fr)_minmax(8rem,0.45fr)_6rem]">
                <div className="sm:col-span-3"><span className="text-sm font-semibold text-primary">{agentDisplayName(backend)}</span><span className="ml-2 break-all text-xs text-muted">{backend}</span></div>
                {agents.includes(key) ? (
                  <label className="min-w-0 space-y-1 text-xs text-secondary">Model
                  <AgentModelSelect label={`Model for ${agentLabel(key)}`} value={models[key] ?? model} options={suggestions[backend]?.options ?? []}
                    currentLabel={currentAgentModelLabel(backend, models[key] ?? model, suggestions[backend]?.options ?? [])}
                    loading={suggestions[backend]?.loading} failed={suggestions[backend]?.failed}
                    onChange={(value) => setModels((current) => ({ ...current, [key]: value }))} />
                  </label>
                ) : (
                  <span className="truncate text-sm text-muted" title="Not in the agent pool; only its limit remains">{model} (not in the pool)</span>
                )}
                <label className="space-y-1 text-xs text-secondary">Reasoning amount
                  <select aria-label={`Reasoning for ${agentLabel(key)}`} title={nativeEffort ? 'Applies to all models using this provider in this project, including implementation and review tasks.' : 'Reasoning variants advertised by this provider.'}
                    value={effortValue} disabled={!agents.includes(key) || reasoning.length === 0} className={`w-full ${INPUT_CLASS}`}
                    onChange={(event) => {
                      if (group) {
                        const variant = group.variants?.find((variant) => variant.effort === event.target.value);
                        if (variant) setModels((current) => ({ ...current, [key]: variant.value }));
                      } else setEfforts((current) => ({ ...current, [backend]: event.target.value }));
                    }}>
                    {reasoning.length > 0 && !reasoning.some((effort) => effort.id === effortValue)
                      && <option value={effortValue}>{effortValue} (current)</option>}
                    {reasoning.length ? reasoning.map((effort) => <option key={effort.id} value={effort.id}>{effort.name}</option>) : <option value="default">Provider default</option>}
                  </select>
                  <p className="text-xs text-muted">{nativeEffort ? `Shared by ${agentDisplayName(backend)} models in this project.` : reasoning.length ? 'For this model.' : 'This provider has no separate task reasoning control.'}</p>
                </label>
                <label className="space-y-1 text-xs text-secondary">Worker limit<input type="number" min={1} aria-label={`Limit for ${agentLabel(key)}`} value={limits[key] ?? ''} placeholder="No limit"
                  onChange={(e) => setLimits((current) => ({ ...current, [key]: e.target.value }))} className={`w-full tabular-nums ${INPUT_CLASS}`} /></label>
              </li>
            );
          })}
        </ul>
      )}
      <p className="mt-2 text-xs text-muted">
        Base capacity is the total number of jobs that can run at once. Leave a model limit blank to share that capacity without an individual cap. Changes apply on the next loop cycle.
      </p>
      {invalid && <p role="alert" className="mt-2 text-xs text-critical">Limits must be whole numbers of at least 1. Leave an agent blank for no limit.</p>}
      {modelError && <p role="alert" className="mt-2 text-xs text-critical">{modelError}</p>}
      {saveError && <p className="mt-2 text-xs text-critical">Failed to save: {saveError}</p>}
      {saved && !saveError && <p className="mt-2 text-xs text-good">Agent settings saved.</p>}
      <button onClick={save} disabled={saving || invalid || modelError != null}
        className="btn-primary mt-3 min-h-11">
        {saving ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        Save agent settings
      </button>
    </section>
  );
}
