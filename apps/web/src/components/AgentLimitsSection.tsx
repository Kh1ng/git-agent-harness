import { useEffect, useRef, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import { gahApi } from '../api/client.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { agentDisplayName } from './LiveAgentsCard.js';

const INPUT_CLASS = 'bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary';

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
function useBackendModels(profile: string, backends: string[]): Record<string, string[]> {
  const [models, setModels] = useState<Record<string, string[]>>({});
  const wanted = backends.join(',');
  useEffect(() => {
    let cancelled = false;
    setModels({});
    for (const backend of wanted ? wanted.split(',') : []) {
      gahApi.getManagerChatModelsForBackend(profile, backend)
        .then((summary) => {
          const names = summary.models.map((model) => (/^agy/.test(backend) ? model.name : model.id)).filter((name) => name !== 'default');
          if (!cancelled) setModels((current) => ({ ...current, [backend]: names }));
        })
        .catch(() => { /* No suggestions; the model can still be typed. */ });
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
  selected: Pick<ProfileSummary, 'max_parallel_workers' | 'max_concurrent_per_model'>;
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
  const [saved, setSaved] = useState(false);
  // Seed once per profile so a refresh after saving keeps in-progress edits.
  const seededProfileRef = useRef<string | null>(null);
  useEffect(() => {
    if (seededProfileRef.current === selectedName) return;
    seededProfileRef.current = selectedName;
    setTotal(selected.max_parallel_workers != null ? String(selected.max_parallel_workers) : '');
    setLimits(Object.fromEntries(Object.entries(caps ?? {}).map(([key, value]) => [key, String(value)])));
    setModels({});
  }, [selectedName, selected.max_parallel_workers, caps]);

  if (!caps) {
    return (
      <section className="card-padded">
        <h3 className="text-sm font-semibold text-primary mb-1">Agent models and limits</h3>
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
    : switches.some((change) => keys.includes(`${change.backend}/${change.to}`))
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
      <h3 id="agent-limits-title" className="text-sm font-semibold text-primary mb-1">Agent models and limits</h3>
      <p className="text-xs text-muted mb-3">
        Which model each subscription runs, and the baseline: how many jobs run at once and how many of them each
        agent may take. Scaling and boosts add to these. Changes apply on the loop's next cycle.
      </p>
      <div className="flex items-center justify-between gap-3 border-b border-subtle pb-2">
        <label htmlFor="agent-limits-total" className="text-sm font-medium text-primary">All agents together</label>
        <input id="agent-limits-total" type="number" min={1} value={total} onChange={(e) => setTotal(e.target.value)} placeholder="1" className={`w-24 tabular-nums ${INPUT_CLASS}`} />
      </div>
      {keys.length === 0 ? (
        <p className="mt-2 text-xs text-muted">No agents are in this profile's routing yet. Add one under "Agent pool" below.</p>
      ) : (
        <ul className="divide-y divide-subtle">
          {keys.map((key) => {
            const { backend, model } = splitAgent(key);
            return (
              <li key={key} className="grid grid-cols-[minmax(5rem,8rem)_minmax(0,1fr)_6rem] items-center gap-3 py-2">
                <span className="truncate text-sm font-medium text-primary" title={key}>{agentDisplayName(backend)}</span>
                {agents.includes(key) ? (
                  <input type="text" list={`agent-models-${backend}`} aria-label={`Model for ${agentLabel(key)}`} value={models[key] ?? model}
                    onChange={(e) => setModels((current) => ({ ...current, [key]: e.target.value }))} className={`min-w-0 ${INPUT_CLASS}`} />
                ) : (
                  <span className="truncate text-sm text-muted" title="Not in the agent pool; only its limit remains">{model} (not in the pool)</span>
                )}
                <input type="number" min={1} aria-label={`Limit for ${agentLabel(key)}`} value={limits[key] ?? ''} placeholder="No limit"
                  onChange={(e) => setLimits((current) => ({ ...current, [key]: e.target.value }))} className={`w-24 tabular-nums ${INPUT_CLASS}`} />
              </li>
            );
          })}
        </ul>
      )}
      {Object.entries(suggestions).map(([backend, names]) => (
        <datalist key={backend} id={`agent-models-${backend}`}>
          {names.map((name) => <option key={name} value={name} />)}
        </datalist>
      ))}
      <p className="mt-2 text-xs text-muted">
        Model names are passed to the agent's own CLI as written. The suggestions come from each CLI; a name it
        does not list can still be typed.
      </p>
      {invalid && <p role="alert" className="mt-2 text-xs text-critical">Limits must be whole numbers of at least 1. Leave an agent blank for no limit.</p>}
      {modelError && <p role="alert" className="mt-2 text-xs text-critical">{modelError}</p>}
      {saveError && <p className="mt-2 text-xs text-critical">Failed to save: {saveError}</p>}
      {saved && !saveError && <p className="mt-2 text-xs text-green-600">Agent models and limits saved.</p>}
      <button onClick={save} disabled={saving || invalid || modelError != null}
        className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed">
        {saving ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        Save models and limits
      </button>
    </section>
  );
}
