import { useEffect, useRef, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { useGahStore } from '../store/gahStore.js';
import type { ProfileSummary } from '@git-agent-harness/contracts';
import { agentDisplayName } from './LiveAgentsCard.js';

const INPUT_CLASS = 'w-24 bg-raised border border-subtle rounded-md px-3 py-1.5 text-sm text-primary tabular-nums';

/** `codex/gpt-5` reads "Codex · gpt-5": the subscription, then the exact model. */
export function agentLabel(key: string): string {
  const slash = key.indexOf('/');
  return slash < 0 ? agentDisplayName(key) : `${agentDisplayName(key.slice(0, slash))} · ${key.slice(slash + 1)}`;
}

/** A whole number of at least 1, or undefined for blank or invalid text. */
function count(text: string): number | undefined {
  const value = Number(text.trim());
  return text.trim() !== '' && Number.isInteger(value) && value >= 1 ? value : undefined;
}

/**
 * The baseline worker counts: how many jobs run at once in total, and how
 * many of them each agent may take. `agents` lists every `backend/model` in
 * the profile's routing, capped or not.
 */
export function AgentLimitsSection({ selectedName, selected, agents }: {
  selectedName: string;
  selected: Pick<ProfileSummary, 'max_parallel_workers' | 'max_concurrent_per_model'>;
  agents: string[];
}) {
  const updateProfile = useGahStore((s) => s.updateProfile);
  const saving = useGahStore((s) => s.profileCrud.updating);
  const saveError = useGahStore((s) => s.profileCrud.updateError);
  const fetchStatus = useGahStore((s) => s.fetchStatus);

  const caps = selected.max_concurrent_per_model;
  const keys = [...new Set([...agents, ...Object.keys(caps ?? {})])].sort();
  const [total, setTotal] = useState('');
  const [limits, setLimits] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState(false);
  // Seed once per profile so a refresh after saving keeps in-progress edits.
  const seededProfileRef = useRef<string | null>(null);
  useEffect(() => {
    if (seededProfileRef.current === selectedName) return;
    seededProfileRef.current = selectedName;
    setTotal(selected.max_parallel_workers != null ? String(selected.max_parallel_workers) : '');
    setLimits(Object.fromEntries(Object.entries(caps ?? {}).map(([key, value]) => [key, String(value)])));
    setSaved(false);
  }, [selectedName, selected.max_parallel_workers, caps]);

  if (!caps) {
    return (
      <section className="card-padded">
        <h3 className="text-sm font-semibold text-primary mb-1">Agent limits</h3>
        <p className="text-xs text-muted">This node's <code>gah</code> does not report per-agent limits yet. Update it to edit them here.</p>
      </section>
    );
  }

  const invalid = (total.trim() !== '' && count(total) === undefined)
    || keys.some((key) => (limits[key] ?? '').trim() !== '' && count(limits[key]) === undefined);
  const save = async () => {
    setSaved(false);
    await updateProfile(selectedName, {
      ...(count(total) !== undefined ? { max_parallel_workers: count(total) } : {}),
      // The command clears first, then sets, so the saved caps are exactly these.
      clear: ['max_concurrent_per_model'],
      max_concurrent: keys.filter((key) => count(limits[key] ?? '') !== undefined).map((key) => `${key}=${count(limits[key])}`),
    });
    await fetchStatus(selectedName, { force: true });
    setSaved(true);
  };

  return (
    <section className="card-padded" aria-labelledby="agent-limits-title">
      <h3 id="agent-limits-title" className="text-sm font-semibold text-primary mb-1">Agent limits</h3>
      <p className="text-xs text-muted mb-3">
        The baseline: how many jobs run at once, and how many of them each agent may take.
        Scaling and boosts below add to these. Changes apply on the loop's next cycle.
      </p>
      <div className="flex items-center justify-between gap-3 border-b border-subtle pb-2">
        <label htmlFor="agent-limits-total" className="text-sm font-medium text-primary">All agents together</label>
        <input id="agent-limits-total" type="number" min={1} value={total} onChange={(e) => setTotal(e.target.value)} placeholder="1" className={INPUT_CLASS} />
      </div>
      {keys.length === 0 ? (
        <p className="mt-2 text-xs text-muted">No agents are in this profile's routing yet. Add one under "Agent pool" below.</p>
      ) : (
        <ul className="divide-y divide-subtle">
          {keys.map((key) => (
            <li key={key} className="flex items-center justify-between gap-3 py-2">
              <label htmlFor={`agent-limit-${key}`} className="min-w-0 truncate text-sm text-secondary" title={key}>{agentLabel(key)}</label>
              <input id={`agent-limit-${key}`} type="number" min={1} value={limits[key] ?? ''} placeholder="No limit"
                onChange={(e) => setLimits((current) => ({ ...current, [key]: e.target.value }))} className={INPUT_CLASS} />
            </li>
          ))}
        </ul>
      )}
      {invalid && <p role="alert" className="mt-2 text-xs text-critical">Limits must be whole numbers of at least 1. Leave an agent blank for no limit.</p>}
      {saveError && <p className="mt-2 text-xs text-critical">Failed to save: {saveError}</p>}
      {saved && !saveError && <p className="mt-2 text-xs text-green-600">Agent limits saved.</p>}
      <button onClick={save} disabled={saving || invalid}
        className="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 bg-accent text-white rounded-md text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed">
        {saving ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <Save size={14} aria-hidden="true" />}
        Save agent limits
      </button>
    </section>
  );
}
